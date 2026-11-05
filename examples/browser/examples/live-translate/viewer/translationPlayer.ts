import type { MoqSession, MoqSubscription } from 'moq-wasm'
import { type MediaCatalogTrack, extractCatalogTracks, parseCatalog } from 'moq-wasm/media'
import { catalogTrackNamespace } from 'moq-wasm/media/catalog'
import { AudioLane } from 'moq-wasm/media/player/audioLane'
import { CatalogFollower } from 'moq-wasm/media/player/catalogFollower'
import { UNOBSERVED_DELIVERIES } from 'moq-wasm/media/player/deliveryObserver'
import { LocLive, type LocObject } from 'moq-wasm/media/player/locLive'
import { PictureStage } from 'moq-wasm/media/player/pictureStage'
import { TextTracks } from 'moq-wasm/media/player/textTrack'
import { type LogLevel, type TrackContext, readGroups, streamId } from 'moq-wasm/media/player/trackContext'
import { getErrorMessage } from '../../../utils/page'
import { translatedNamespace } from '../liveTranslate'
import { type LanguageOption, ORIGINAL_LANGUAGE, languageOptions } from './languages'

/// Every track plays this long after its capture timestamp, so a translation
/// that arrives a few seconds after the speech still plays with its picture.
export const PLAYBACK_DELAY_MS = 5_000
const CROSSFADE_TIME_CONSTANT_SECONDS = 0.08
const RETIRED_LANE_CLOSE_DELAY_MS = 500
const MONITORED_ORIGINAL_VOLUME = 0.5

type Subscribed = { handle: MoqSubscription; name: string }

type Translation = {
  lang: string
  selectedAtMs: number
  subscribed: Subscribed | undefined
  lane: AudioLane
  crossfadeTimer?: ReturnType<typeof setTimeout>
  closed?: boolean
}

export type TranslationPlayerCallbacks = {
  onStateChange(): void
  onLog(level: LogLevel, message: string): void
}

export type TranslationPlayerStats = {
  language: string
  pendingLanguage: string | undefined
  /// How long the last switch to a translation took, from selecting it to hearing it.
  lastSwitchSeconds: number | undefined
  forwardedAudioObjects: Record<string, number>
  frameSize: { width: number; height: number } | undefined
  receivedVideoKbps: number
  droppedLateAudio: number
  droppedLateVideo: number
  playingAudioMicros: number | undefined
  syncOffsetMs: number | undefined
  translationOffsetMs: number | undefined
  viewerDelayMs: number | undefined
}

/// Plays a live translation broadcast: the source's video and original audio
/// on one clock, `playbackDelayMs` behind capture, and the selected
/// language's translation crossfaded in on a lane of its own once it has
/// caught up with the clock. The original audio stays subscribed, so going
/// back to it is a crossfade too.
export class TranslationPlayer {
  private readonly context: TrackContext
  private readonly textTracks: TextTracks
  private readonly catalog: CatalogFollower
  private readonly stage: PictureStage
  private readonly locLive: LocLive
  private readonly forwardedAudioObjects: Record<string, number> = {}
  private changes: Promise<void> = Promise.resolve()
  private started = false
  private video: Subscribed | undefined
  private original: Subscribed | undefined
  private translation: Translation | undefined
  private incoming: Translation | undefined
  private droppedLateTranslatedAudio = 0
  private monitorOriginal = false
  private lastSwitchSeconds: number | undefined
  languages: LanguageOption[] = []

  constructor(
    container: HTMLElement,
    private readonly callbacks: TranslationPlayerCallbacks
  ) {
    this.context = {
      session: undefined,
      namespace: [],
      observer: UNOBSERVED_DELIVERIES,
      log: (level, message) => callbacks.onLog(level, message)
    }
    this.textTracks = new TextTracks(this.context)
    this.catalog = new CatalogFollower(
      this.textTracks,
      (text) => void this.queue(() => this.applyCatalog(text)),
      (done) => callbacks.onLog('warn', `the catalog ended (status ${done.code})`)
    )
    this.stage = new PictureStage(container)
    this.locLive = new LocLive(this.stage.liveVideo, this.stage.liveCanvas, undefined, {
      onPresented: (picture) => this.stage.show(picture),
      onFrameShown: () => undefined
    })
    this.locLive.setBufferPolicy({ minimumMs: PLAYBACK_DELAY_MS, maximumMs: PLAYBACK_DELAY_MS })
    this.locLive.playout.setCatchUp('off')
    this.locLive.playout.setLateSamples('drop')
    this.stage.show(this.locLive.picture.element)
  }

  get language(): string {
    return this.translation?.lang ?? ORIGINAL_LANGUAGE
  }

  get pendingLanguage(): string | undefined {
    return this.incoming?.lang
  }

  stats(): TranslationPlayerStats {
    const live = this.locLive.stats()
    const lanes = [this.translation, this.incoming].filter((translation) => translation !== undefined)
    return {
      language: this.language,
      pendingLanguage: this.pendingLanguage,
      lastSwitchSeconds: this.lastSwitchSeconds,
      forwardedAudioObjects: { ...this.forwardedAudioObjects },
      frameSize: live.frameSize,
      receivedVideoKbps: live.receivedKbps,
      droppedLateAudio:
        this.locLive.playout.droppedLateAudio +
        this.droppedLateTranslatedAudio +
        lanes.reduce((sum, { lane }) => sum + lane.droppedLate, 0),
      droppedLateVideo: this.locLive.playout.droppedLateVideo,
      playingAudioMicros: this.translation
        ? this.translation.lane.positionMicros()
        : this.locLive.playout.audioPositionMicros(),
      syncOffsetMs: live.syncOffsetMs,
      translationOffsetMs: this.translationOffsetMs(),
      viewerDelayMs: live.viewerDelayMs
    }
  }

  private translationOffsetMs(): number | undefined {
    const translatedMicros = this.translation?.lane.positionMicros()
    const originalMicros = this.locLive.playout.audioPositionMicros()
    return translatedMicros === undefined || originalMicros === undefined
      ? undefined
      : (translatedMicros - originalMicros) / 1_000
  }

  async start(session: MoqSession, broadcastId: string): Promise<void> {
    await this.queue(async () => {
      this.context.session = session
      this.context.namespace = translatedNamespace(broadcastId)
      this.started = true
    })
    await this.catalog.follow()
  }

  stop(): Promise<void> {
    return this.queue(async () => {
      this.started = false
      for (const translation of [this.translation, this.incoming]) {
        await this.close(translation)
      }
      this.translation = this.incoming = undefined
      for (const subscribed of [this.video, this.original]) {
        await this.unsubscribe(subscribed)
      }
      this.video = this.original = undefined
      await this.textTracks.unsubscribeAll()
      this.catalog.reset()
      this.locLive.reset()
      this.locLive.playout.setVolume(1)
      this.context.session = undefined
      this.languages = []
      this.callbacks.onStateChange()
    })
  }

  setMonitorOriginal(monitor: boolean): void {
    this.monitorOriginal = monitor
    if (this.translation) {
      this.locLive.playout.setVolume(this.originalVolumeUnderTranslation(), CROSSFADE_TIME_CONSTANT_SECONDS)
    }
  }

  selectLanguage(lang: string): Promise<void> {
    return this.queue(async () => {
      if (lang === this.incoming?.lang) {
        return
      }
      const abandoned = this.incoming
      this.incoming = undefined
      await this.close(abandoned)
      const option = this.languages.find((candidate) => candidate.lang === lang)
      if (!option || lang === this.language) {
        this.callbacks.onStateChange()
        return
      }
      if (lang === ORIGINAL_LANGUAGE) {
        this.locLive.playout.setVolume(1, CROSSFADE_TIME_CONSTANT_SECONDS)
        this.retire(this.translation)
        this.translation = undefined
        this.callbacks.onLog('info', `switched audio to ${lang}`)
        this.callbacks.onStateChange()
        return
      }
      await this.subscribeTranslation(option)
      this.callbacks.onStateChange()
    })
  }

  private async subscribeTranslation(option: LanguageOption): Promise<void> {
    const translation: Translation = {
      lang: option.lang,
      selectedAtMs: performance.now(),
      subscribed: undefined,
      lane: new AudioLane(option.track, this.locLive.playout, (dueMs) => this.crossfadeAt(translation, dueMs))
    }
    this.incoming = translation
    try {
      translation.subscribed = await this.subscribe(option.track, (groupId, object) => {
        translation.lane.push(groupId, object)
        this.countForwarded(option.track.name)
      })
    } catch (error) {
      if (this.incoming === translation) {
        this.incoming = undefined
      }
      translation.lane.close()
      this.callbacks.onLog('error', `${option.label} is unavailable: ${getErrorMessage(error)}`)
    }
  }

  private crossfadeAt(translation: Translation, dueMs: number): void {
    translation.crossfadeTimer = setTimeout(() => this.crossfade(translation), Math.max(0, dueMs - performance.now()))
  }

  private crossfade(translation: Translation): void {
    translation.crossfadeTimer = undefined
    if (this.incoming !== translation) {
      return
    }
    this.locLive.playout.setVolume(this.originalVolumeUnderTranslation(), CROSSFADE_TIME_CONSTANT_SECONDS)
    translation.lane.fadeTo(1, CROSSFADE_TIME_CONSTANT_SECONDS)
    this.retire(this.translation)
    this.translation = translation
    this.incoming = undefined
    this.lastSwitchSeconds = (performance.now() - translation.selectedAtMs) / 1_000
    this.callbacks.onLog(
      'info',
      `switched audio to ${translation.lang} ${this.lastSwitchSeconds.toFixed(1)} s after selecting it`
    )
    this.callbacks.onStateChange()
  }

  private originalVolumeUnderTranslation(): number {
    return this.monitorOriginal ? MONITORED_ORIGINAL_VOLUME : 0
  }

  private retire(translation: Translation | undefined): void {
    if (!translation) {
      return
    }
    translation.lane.fadeTo(0, CROSSFADE_TIME_CONSTANT_SECONDS)
    setTimeout(() => void this.queue(() => this.close(translation)), RETIRED_LANE_CLOSE_DELAY_MS)
  }

  private async close(translation: Translation | undefined): Promise<void> {
    if (!translation || translation.closed) {
      return
    }
    translation.closed = true
    clearTimeout(translation.crossfadeTimer)
    this.droppedLateTranslatedAudio += translation.lane.droppedLate
    translation.lane.close()
    await this.unsubscribe(translation.subscribed)
    translation.subscribed = undefined
  }

  private queue(change: () => Promise<void>): Promise<void> {
    const run = this.changes.then(change)
    this.changes = run.catch((error) => this.callbacks.onLog('error', getErrorMessage(error)))
    return run
  }

  private async applyCatalog(text: string): Promise<void> {
    if (!this.started) {
      return
    }
    const catalog = parseCatalog(text)
    const [videoTrack] = extractCatalogTracks(catalog, 'video').filter(isLocTrack)
    this.languages = languageOptions(extractCatalogTracks(catalog, 'audio').filter(isLocTrack))
    if (!this.video && videoTrack) {
      this.locLive.configureTrack('video', videoTrack)
      this.video = await this.subscribe(videoTrack, (groupId, object) => this.locLive.push('video', groupId, object))
    }
    const original = this.languages.find((option) => option.lang === ORIGINAL_LANGUAGE)?.track
    if (!this.original && original) {
      this.locLive.configureTrack('audio', original)
      this.original = await this.subscribe(original, (groupId, object) => {
        this.locLive.push('audio', groupId, object)
        this.countForwarded(original.name)
      })
    }
    this.callbacks.onStateChange()
  }

  private countForwarded(trackName: string): void {
    this.forwardedAudioObjects[trackName] = (this.forwardedAudioObjects[trackName] ?? 0) + 1
  }

  private async subscribe(
    track: MediaCatalogTrack,
    onObject: (groupId: bigint, object: LocObject) => void
  ): Promise<Subscribed> {
    const session = this.context.session
    if (!session) {
      throw new Error('the player has not started')
    }
    const namespace = catalogTrackNamespace(track, this.context.namespace)
    const handle = await session.subscribe(namespace, track.name, { forward: true })
    void readGroups(this.context, streamId(), handle, onObject)
    this.callbacks.onLog('info', `subscribed ${namespace.join('/')}/${track.name}`)
    return { handle, name: track.name }
  }

  private async unsubscribe(subscribed: Subscribed | undefined): Promise<void> {
    if (!subscribed) {
      return
    }
    if (this.context.session?.isOpen) {
      await subscribed.handle.close()
    }
    this.callbacks.onLog('info', `unsubscribed ${subscribed.name}`)
  }
}

function isLocTrack(track: MediaCatalogTrack): boolean {
  return track.packaging !== 'cmaf'
}
