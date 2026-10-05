import { MoqtClientWrapper } from '@moqt/moqtClient'
import { type LiveStats, LocLive } from '@player/locLive'
import { createPictureCanvas, createPictureVideo } from '@player/pictureElements'
import type { MediaKind } from '@player/trackContext'
import type { TrackMediaConfig } from '../types/catalog'
import { DEFAULT_PLAYOUT_SETTINGS, type PlayoutSettings } from '../types/playout'
import { isScreenShareTrackName } from '../utils/catalogTrackName'

export type RemoteVideoSource = 'camera' | 'screenshare'
export type RemotePlaybackStats = Partial<Record<RemoteVideoSource, LiveStats>>

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
}

/// The camera pipeline also carries the member's audio; a screen share plays
/// on its own pipeline with the picture as the clock's master.
type MemberPipelines = Partial<Record<RemoteVideoSource, LocLive>>

export class MediaSubscriber {
  private handlers: MediaSubscriberHandlers = {}
  private readonly pipelines = new Map<string, MemberPipelines>()
  private readonly registrations = new Map<bigint, TrackRegistration>()
  private readonly playoutSettingsByUserId = new Map<string, PlayoutSettings>()

  constructor(private readonly client: MoqtClientWrapper) {}

  setHandlers(handlers: MediaSubscriberHandlers): void {
    this.handlers = handlers
  }

  registerVideoTrack(userId: string, trackName: string, trackAlias: bigint, config?: TrackMediaConfig): void {
    const source: RemoteVideoSource = isScreenShareTrackName(trackName) ? 'screenshare' : 'camera'
    this.register(trackAlias, { userId, source, kind: 'video', trackName, config })
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
    const remaining = this.registrationsOf(userId, source)
    if (remaining.length === 0) {
      this.closePipeline(userId, source)
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

  private attach(trackAlias: bigint, { userId, source, kind, trackName, config }: TrackRegistration): void {
    const pipeline = this.ensurePipeline(userId, source)
    pipeline.configureTrack(kind, {
      name: trackName,
      label: trackName,
      codec: config?.codec,
      initData: config?.initData,
      samplerate: config?.samplerate,
      channelConfig: config?.channelConfig
    })
    this.client.setOnSubgroupObjectHandler(trackAlias, (groupId, message) => pipeline.push(kind, groupId, message))
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
    const { [source]: _closed, ...rest } = pipelines
    if (Object.keys(rest).length === 0) {
      this.pipelines.delete(userId)
    } else {
      this.pipelines.set(userId, rest)
    }
    this.handlers.onRemotePictureClosed?.(userId, source)
  }

  private registrationsOf(userId: string, source: RemoteVideoSource): [bigint, TrackRegistration][] {
    return Array.from(this.registrations).filter(
      ([, registration]) => registration.userId === userId && registration.source === source
    )
  }
}

function applyPlayoutSettings(pipeline: LocLive, settings: PlayoutSettings): void {
  pipeline.setBufferPolicy(settings.policy)
  pipeline.playout.setCatchUp(settings.catchUp)
}
