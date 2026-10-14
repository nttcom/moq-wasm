import { MoqtClientWrapper } from '@moqt/moqtClient'
import { type LiveStats, LocLive } from '@player/locLive'
import { createPictureCanvas, createPictureVideo } from '@player/pictureElements'
import { AdaptiveBitrate, type Rung, capLadderToHeight } from '@player/adaptiveBitrate'
import { RenditionSplice } from '@player/renditionSplice'
import { monotonicUnixMicros } from '../../../../utils/media/clock'
import { readLocHeader } from '../../../../utils/media/loc'
import type { SubgroupObjectMessageWithLoc } from '@moqt/subscriptionStateManager'
import type { MediaKind } from '@player/trackContext'
import type { TrackMediaConfig } from '../types/catalog'
import { DEFAULT_PLAYOUT_SETTINGS, type PlayoutSettings } from '../types/playout'
import { isScreenShareTrackName } from '../utils/catalogTrackName'

export type RemoteVideoSource = 'camera' | 'screenshare'
export type RemotePlaybackStats = Partial<Record<RemoteVideoSource, LiveStats>>
export type RemotePictures = Partial<Record<RemoteVideoSource, HTMLElement>>

const ADAPT_INTERVAL_MS = 500
/// After asking for another track, the next choice waits for the switch.
const SWITCH_WAIT_MS = 3_000

export type VideoAdaptation = { ladder: Rung[]; onPreferred: (trackName: string) => void }

export interface MediaSubscriberHandlers {
  onRemotePicture?: (userId: string, source: RemoteVideoSource, picture: HTMLElement) => void
  onRemotePictureClosed?: (userId: string, source: RemoteVideoSource) => void
}

type TrackRegistration = {
  userId: string
  source: RemoteVideoSource
  kind: MediaKind
  trackName: string
  config: TrackMediaConfig | undefined
  /// Unsubscribes the track once another video track has taken over its picture.
  retire?: () => void
  adaptation?: VideoAdaptation
  newestGroupId?: bigint
}

type PictureAdaptation = {
  bitrate: AdaptiveBitrate
  resizeObserver: ResizeObserver
  heightPx?: number
  nextChoiceAtMs: number
}

/// A video track of a picture being replaced by another one of the same codec:
/// the outgoing track plays until the incoming one starts a group.
type VideoSwitch = { splice: RenditionSplice; outgoing: bigint; incoming: bigint }

/// The camera pipeline also carries the member's audio; a screen share plays
/// on its own pipeline with the picture as the clock's master.
type MemberPipelines = Partial<Record<RemoteVideoSource, LocLive>>

export class MediaSubscriber {
  private handlers: MediaSubscriberHandlers = {}
  private readonly pipelines = new Map<string, MemberPipelines>()
  private readonly registrations = new Map<bigint, TrackRegistration>()
  private readonly videoSwitches = new Map<string, VideoSwitch>()
  private readonly pictureAdaptations = new Map<string, PictureAdaptation>()
  private readonly playoutSettingsByUserId = new Map<string, PlayoutSettings>()

  constructor(private readonly client: MoqtClientWrapper) {}

  setHandlers(handlers: MediaSubscriberHandlers): void {
    this.handlers = handlers
  }

  /// A video track registered while the picture plays another one takes over
  /// without a gap; call `canReplaceVideo` first and unsubscribe the playing
  /// track when it returns false. With `adaptation`, the picture asks for the
  /// ladder track that fits its shown height and the delivery: no taller than
  /// the picture, lower when the capture-to-arrival lag rises (see `AdaptiveBitrate`).
  registerVideoTrack(
    userId: string,
    trackName: string,
    trackAlias: bigint,
    config: TrackMediaConfig | undefined,
    retire: () => void,
    adaptation?: VideoAdaptation
  ): void {
    const source: RemoteVideoSource = isScreenShareTrackName(trackName) ? 'screenshare' : 'camera'
    const outgoing = this.videoRegistration(userId, source)
    if (outgoing && !this.registrations.has(trackAlias)) {
      const [outgoingAlias, outgoingRegistration] = outgoing
      this.videoSwitches.set(pictureKey(userId, source), {
        splice: new RenditionSplice(outgoingRegistration.newestGroupId),
        outgoing: outgoingAlias,
        incoming: trackAlias
      })
    }
    this.register(trackAlias, { userId, source, kind: 'video', trackName, config, retire, adaptation })
  }

  /// The video decoder keeps the configuration of the first track it decodes,
  /// so only a track of the same codec can take over a playing picture.
  canReplaceVideo(trackAlias: bigint, config: TrackMediaConfig | undefined): boolean {
    const registration = this.registrations.get(trackAlias)
    return registration?.kind === 'video' && registration.config?.codec === config?.codec
  }

  registerAudioTrack(userId: string, trackName: string, trackAlias: bigint, config?: TrackMediaConfig): void {
    this.register(trackAlias, { userId, source: 'camera', kind: 'audio', trackName, config })
  }

  unregisterTrack(trackAlias: bigint): void {
    this.client.clearSubgroupObjectHandler(trackAlias)
    const registration = this.registrations.get(trackAlias)
    if (!registration) {
      return
    }
    this.registrations.delete(trackAlias)
    const { userId, source, kind } = registration
    const remaining = Array.from(this.registrations).filter(
      ([, other]) => other.userId === userId && other.source === source
    )
    if (remaining.length === 0) {
      this.closePipeline(userId, source)
      return
    }
    if (kind === 'video' && this.settleVideoSwitch(userId, source, trackAlias)) {
      return
    }
    // The video worker keeps the decoder state of the track that left, so the
    // next video track starts on a fresh pipeline; only the audio warm-up repeats.
    if (kind === 'video') {
      this.closePipeline(userId, source)
      for (const [alias, remainingRegistration] of remaining) {
        this.attach(alias, remainingRegistration)
      }
    }
  }

  setPlayoutSettings(userId: string, settings: PlayoutSettings): void {
    this.playoutSettingsByUserId.set(userId, settings)
    for (const pipeline of Object.values(this.pipelines.get(userId) ?? {})) {
      applyPlayoutSettings(pipeline, settings)
    }
  }

  stats(userId: string): RemotePlaybackStats {
    const pipelines = this.pipelines.get(userId) ?? {}
    return {
      camera: pipelines.camera?.stats(),
      screenshare: pipelines.screenshare?.stats()
    }
  }

  dispose(): void {
    for (const trackAlias of Array.from(this.registrations.keys())) {
      this.client.clearSubgroupObjectHandler(trackAlias)
    }
    this.registrations.clear()
    for (const [userId, pipelines] of this.pipelines) {
      for (const source of Object.keys(pipelines) as RemoteVideoSource[]) {
        this.closePipeline(userId, source)
      }
    }
    this.playoutSettingsByUserId.clear()
    this.handlers = {}
  }

  private register(trackAlias: bigint, registration: TrackRegistration): void {
    if (this.registrations.has(trackAlias)) {
      return
    }
    this.registrations.set(trackAlias, registration)
    const pipeline = this.pipelines.get(registration.userId)?.[registration.source]
    // Audio joining a running picture warms up again so the relay's cached
    // burst does not drag the clock back as the new master.
    if (pipeline && registration.kind === 'audio') {
      pipeline.reset()
    }
    this.attach(trackAlias, registration)
  }

  private attach(trackAlias: bigint, registration: TrackRegistration): void {
    const { userId, source, kind } = registration
    const pipeline = this.ensurePipeline(userId, source)
    if (this.videoSwitches.get(pictureKey(userId, source))?.incoming !== trackAlias) {
      configurePipeline(pipeline, registration)
    }
    if (kind === 'audio') {
      this.client.setOnSubgroupObjectHandler(trackAlias, (groupId, message) => pipeline.push(kind, groupId, message))
      return
    }
    this.client.setOnSubgroupObjectHandler(trackAlias, (groupId, message) =>
      this.pushVideo(trackAlias, registration, pipeline, groupId, message)
    )
  }

  private pushVideo(
    trackAlias: bigint,
    registration: TrackRegistration,
    pipeline: LocLive,
    groupId: bigint,
    message: SubgroupObjectMessageWithLoc
  ): void {
    const key = pictureKey(registration.userId, registration.source)
    const videoSwitch = this.videoSwitches.get(key)
    if (videoSwitch?.outgoing === trackAlias && videoSwitch.splice.acceptOutgoing(groupId) === 'retire') {
      this.retireOutgoing(key, videoSwitch)
      return
    }
    if (videoSwitch?.incoming === trackAlias) {
      const verdict = videoSwitch.splice.acceptIncoming(groupId, message.objectId)
      if (verdict.start) {
        configurePipeline(pipeline, registration)
        this.pictureAdaptations.get(key)?.bitrate.restart(nowMs())
      }
      if (verdict.retireOutgoing) {
        this.retireOutgoing(key, videoSwitch)
      }
      if (!verdict.play) {
        return
      }
    }
    if (registration.newestGroupId === undefined || groupId > registration.newestGroupId) {
      registration.newestGroupId = groupId
    }
    pipeline.push('video', groupId, message)
    this.adapt(key, registration, message)
  }

  private adapt(key: string, registration: TrackRegistration, message: SubgroupObjectMessageWithLoc): void {
    const picture = this.pictureAdaptations.get(key)
    const adaptation = registration.adaptation
    if (!picture || !adaptation) {
      return
    }
    const arrivedAtMs = nowMs()
    const captureMicros = readLocHeader(message.locHeader).captureTimestampMicros
    picture.bitrate.observe({
      arrivedAtMs,
      bytes: message.objectPayloadLength,
      lagMs: captureMicros === undefined ? undefined : arrivedAtMs - captureMicros / 1_000
    })
    if (this.videoSwitches.has(key) || arrivedAtMs < picture.nextChoiceAtMs) {
      return
    }
    picture.nextChoiceAtMs = arrivedAtMs + ADAPT_INTERVAL_MS
    const current = registration.trackName
    if (!adaptation.ladder.some((rung) => rung.name === current)) {
      return
    }
    const ladder = capLadderToHeight(adaptation.ladder, picture.heightPx)
    const choice = ladder.some((rung) => rung.name === current)
      ? picture.bitrate.decide(arrivedAtMs, ladder, current, false)
      : ladder.at(-1)?.name
    if (choice && choice !== current) {
      picture.nextChoiceAtMs = arrivedAtMs + SWITCH_WAIT_MS
      adaptation.onPreferred(choice)
    }
  }

  private retireOutgoing(key: string, videoSwitch: VideoSwitch): void {
    if (this.videoSwitches.get(key) !== videoSwitch) {
      return
    }
    this.videoSwitches.delete(key)
    this.registrations.get(videoSwitch.outgoing)?.retire?.()
  }

  /// Returns true when the pipeline stays: the track that left took part in a
  /// switch, or another video track already plays on it. An incoming track
  /// that had not started is configured now.
  private settleVideoSwitch(userId: string, source: RemoteVideoSource, trackAlias: bigint): boolean {
    const key = pictureKey(userId, source)
    const videoSwitch = this.videoSwitches.get(key)
    if (videoSwitch) {
      this.videoSwitches.delete(key)
      const incoming = this.registrations.get(videoSwitch.incoming)
      const pipeline = this.pipelines.get(userId)?.[source]
      if (videoSwitch.outgoing === trackAlias && incoming && pipeline && videoSwitch.splice.start === undefined) {
        configurePipeline(pipeline, incoming)
      }
      return true
    }
    return this.videoRegistration(userId, source) !== undefined
  }

  /// The shown height is in device pixels, the unit a rendition's height compares to.
  private observePicture(key: string, element: HTMLElement): void {
    const bitrate = new AdaptiveBitrate()
    bitrate.restart(nowMs())
    const picture: PictureAdaptation = {
      bitrate,
      nextChoiceAtMs: 0,
      resizeObserver: new ResizeObserver(([entry]) => {
        const heightPx = entry.contentRect.height * window.devicePixelRatio
        picture.heightPx = heightPx > 0 ? heightPx : undefined
      })
    }
    picture.resizeObserver.observe(element)
    this.pictureAdaptations.set(key, picture)
  }

  private videoRegistration(userId: string, source: RemoteVideoSource): [bigint, TrackRegistration] | undefined {
    return Array.from(this.registrations).find(
      ([, registration]) =>
        registration.userId === userId && registration.source === source && registration.kind === 'video'
    )
  }

  private ensurePipeline(userId: string, source: RemoteVideoSource): LocLive {
    const pipelines = this.pipelines.get(userId) ?? {}
    const existing = pipelines[source]
    if (existing) {
      return existing
    }
    const testId = source === 'camera' ? `member-video-${userId}` : `member-screenshare-video-${userId}`
    const pipeline = new LocLive(
      createPictureVideo({ testId, muted: true }),
      createPictureCanvas(`${testId}-canvas`),
      undefined,
      { onPresented: () => {}, onFrameShown: () => {} }
    )
    pipeline.picture.element.hidden = false
    this.observePicture(pictureKey(userId, source), pipeline.picture.element)
    applyPlayoutSettings(pipeline, this.playoutSettingsByUserId.get(userId) ?? DEFAULT_PLAYOUT_SETTINGS)
    this.pipelines.set(userId, { ...pipelines, [source]: pipeline })
    this.handlers.onRemotePicture?.(userId, source, pipeline.picture.element)
    return pipeline
  }

  private closePipeline(userId: string, source: RemoteVideoSource): void {
    const pipelines = this.pipelines.get(userId)
    const pipeline = pipelines?.[source]
    if (!pipelines || !pipeline) {
      return
    }
    pipeline.dispose()
    const key = pictureKey(userId, source)
    this.pictureAdaptations.get(key)?.resizeObserver.disconnect()
    this.pictureAdaptations.delete(key)
    const { [source]: _closed, ...rest } = pipelines
    if (Object.keys(rest).length === 0) {
      this.pipelines.delete(userId)
    } else {
      this.pipelines.set(userId, rest)
    }
    this.handlers.onRemotePictureClosed?.(userId, source)
  }
}

function nowMs(): number {
  return monotonicUnixMicros() / 1_000
}

function pictureKey(userId: string, source: RemoteVideoSource): string {
  return `${userId}:${source}`
}

function configurePipeline(pipeline: LocLive, { kind, trackName, config }: TrackRegistration): void {
  pipeline.configureTrack(kind, {
    name: trackName,
    label: trackName,
    codec: config?.codec,
    initData: config?.initData,
    samplerate: config?.samplerate,
    channelConfig: config?.channelConfig
  })
}

function applyPlayoutSettings(pipeline: LocLive, settings: PlayoutSettings): void {
  pipeline.setBufferPolicy(settings.policy)
  pipeline.playout.setCatchUp(settings.catchUp)
}
