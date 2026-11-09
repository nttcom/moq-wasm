import { type MoqSession, type MoqSubscription, type PublishDone, RequestError, RequestErrorCode } from 'moq-wasm'
import { type MediaCatalogTrack, extractCatalogTracks, parseCatalog } from 'moq-wasm/media'
import { catalogTrackNamespace } from 'moq-wasm/media/catalog'
import { readLocHeader } from 'moq-wasm/media/internal/loc'
import { AudioLane } from 'moq-wasm/media/player/audioLane'
import { CaptionLane } from './captionLane'
import { CatalogFollower } from 'moq-wasm/media/player/catalogFollower'
import { UNOBSERVED_DELIVERIES } from 'moq-wasm/media/player/deliveryObserver'
import { LocLive, type LocObject } from 'moq-wasm/media/player/locLive'
import { PictureStage } from 'moq-wasm/media/player/pictureStage'
import { TextTracks } from 'moq-wasm/media/player/textTrack'
import { type LogLevel, type TrackContext, readJoinedObjects, streamId } from 'moq-wasm/media/player/trackContext'
import { getErrorMessage } from '../../../utils/page'
import { type LanguageOption, ORIGINAL_LANGUAGE, languageOptions } from './languages'

/// Every track plays this long after its capture timestamp, so a translation
/// that arrives a few seconds after the speech still plays with its picture.
/// It must exceed Gemini's delay (about 3.7 s); the rest absorbs the jitter.
const DEFAULT_PLAYBACK_DELAY_MS = 5_000
const CROSSFADE_TIME_CONSTANT_SECONDS = 0.08
const BROADCAST_RETRY_MS = 2_000
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
  /// The caption line of the language heard; empty when there is none.
  onCaption(text: string): void
}

export type TranslationPlayerOptions = {
  playbackDelayMs?: number
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
  /// Capture-to-arrival delay of the window's fastest sample; with `bufferMs` it makes up `viewerDelayMs`.
  arrivalDelayMs: number | undefined
  bufferMs: number | undefined
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
  private followed = false
  private retryTimer: ReturnType<typeof setTimeout> | undefined
  private video: Subscribed | undefined
  private original: Subscribed | undefined
  private translation: Translation | undefined
  private incoming: Translation | undefined
  private droppedLateTranslatedAudio = 0
  private monitorOriginal = false
  private lastSwitchSeconds: number | undefined
  private notice: string | undefined
  languages: LanguageOption[] = []
  private transcriptTracks = new Map<string, MediaCatalogTrack>()
  private captions: { lang: string; subscribed: Subscribed; lane: CaptionLane } | undefined

  readonly playbackDelayMs: number

  constructor(
    container: HTMLElement,
    private readonly callbacks: TranslationPlayerCallbacks,
    options: TranslationPlayerOptions = {}
  ) {
    this.playbackDelayMs = options.playbackDelayMs ?? DEFAULT_PLAYBACK_DELAY_MS
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
      (done) => {
        callbacks.onLog('warn', `the catalog ended (status ${done.code})`)
        void this.queue(async () => {
          this.catalog.reset()
          this.followed = false
          this.callbacks.onStateChange()
        })
          .then(() => (this.started ? this.followBroadcast() : undefined))
          .catch((error: unknown) => callbacks.onLog('error', `following the catalog again: ${getErrorMessage(error)}`))
      }
    )
    this.stage = new PictureStage(container)
    this.locLive = new LocLive(this.stage.liveVideo, this.stage.liveCanvas, undefined, {
      onPresented: (picture) => this.stage.show(picture),
      onFrameShown: () => undefined
    })
    this.locLive.setBufferPolicy({ minimumMs: this.playbackDelayMs, maximumMs: this.playbackDelayMs })
    this.locLive.playout.setCatchUp('off')
    this.locLive.playout.setLateSamples('drop')
    this.stage.show(this.locLive.picture.element)
  }

  get watching(): boolean {
    return this.started
  }

  /// Started, but the broadcast's translated namespace is not published yet.
  get waiting(): boolean {
    return this.started && !this.followed
  }

  get language(): string {
    return this.translation?.lang ?? ORIGINAL_LANGUAGE
  }

  get pendingLanguage(): string | undefined {
    return this.incoming?.lang
  }

  /// Why the chosen language is not heard: its SUBSCRIBE failed or the
  /// translator ended it. Cleared by the next choice.
  get languageNotice(): string | undefined {
    return this.notice
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
      viewerDelayMs: live.viewerDelayMs,
      arrivalDelayMs: live.fastestArrivalDelayMs,
      bufferMs: live.bufferMs
    }
  }

  private translationOffsetMs(): number | undefined {
    const translatedMicros = this.translation?.lane.positionMicros()
    const originalMicros = this.locLive.playout.audioPositionMicros()
    return translatedMicros === undefined || originalMicros === undefined
      ? undefined
      : (translatedMicros - originalMicros) / 1_000
  }

  async start(session: MoqSession, namespace: string[]): Promise<void> {
    await this.queue(async () => {
      this.context.session = session
      this.context.namespace = namespace
      this.started = true
      this.followed = false
    })
    await this.followBroadcast()
  }

  /// The relay answers the SUBSCRIBE of a namespace nobody publishes with
  /// TRACK_DOES_NOT_EXIST: the link was opened before the streamer started, or
  /// the translator has not announced the broadcast yet. The catalog is then
  /// tried again every 2 s while the player runs. Any other failure of the
  /// first attempt is the caller's; a later one is logged.
  private async followBroadcast(): Promise<void> {
    try {
      await this.catalog.follow()
      this.followed = true
      this.callbacks.onStateChange()
    } catch (error) {
      if (!this.started || !(error instanceof RequestError && error.errorCode === RequestErrorCode.TrackDoesNotExist)) {
        throw error
      }
      this.callbacks.onStateChange()
      this.retryTimer = setTimeout(() => {
        this.retryTimer = undefined
        this.followBroadcast().catch((error: unknown) =>
          this.callbacks.onLog('error', `waiting for the broadcast: ${getErrorMessage(error)}`)
        )
      }, BROADCAST_RETRY_MS)
    }
  }

  stop(): Promise<void> {
    return this.queue(async () => {
      this.started = false
      this.followed = false
      clearTimeout(this.retryTimer)
      this.retryTimer = undefined
      for (const translation of [this.translation, this.incoming]) {
        await this.close(translation)
      }
      this.translation = this.incoming = undefined
      for (const subscribed of [this.video, this.original]) {
        await this.unsubscribe(subscribed)
      }
      this.video = this.original = undefined
      await this.showCaptions(undefined)
      this.transcriptTracks.clear()
      await this.textTracks.unsubscribeAll()
      this.catalog.reset()
      this.locLive.reset()
      this.locLive.playout.setVolume(1)
      this.context.session = undefined
      this.languages = []
      this.notice = undefined
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
      this.notice = undefined
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
        await this.showCaptions(ORIGINAL_LANGUAGE)
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
      translation.subscribed = await this.subscribe(option.track, (groupId, object, cached) => {
        translation.lane.push(groupId, object, cached)
        this.countForwarded(option.track.name)
      })
      void translation.subscribed.handle.done.then((done) => {
        if (done) {
          void this.queue(() => this.translationEnded(translation, done))
        }
      })
    } catch (error) {
      if (this.incoming === translation) {
        this.incoming = undefined
      }
      translation.lane.close()
      this.notice = `${option.label} は利用できません: ${error instanceof RequestError ? error.reasonPhrase : getErrorMessage(error)}`
      this.callbacks.onLog('error', `${option.label} is unavailable: ${getErrorMessage(error)}`)
    }
  }

  /// The translator ends a language after its time limit, when its open
  /// hours close or when its Gemini session gives up, so the original audio
  /// comes back on its own.
  private async translationEnded(translation: Translation, done: PublishDone): Promise<void> {
    if (translation.closed) {
      return
    }
    const reason = done.reason || `status ${done.code}`
    const label = this.languages.find((option) => option.lang === translation.lang)?.label ?? translation.lang
    this.notice = `${label} の翻訳が止まりました: ${reason}`
    this.callbacks.onLog('warn', `${translation.lang} ended by the translator: ${reason}`)
    if (this.translation === translation) {
      this.locLive.playout.setVolume(1, CROSSFADE_TIME_CONSTANT_SECONDS)
      this.translation = undefined
      await this.showCaptions(ORIGINAL_LANGUAGE)
    }
    if (this.incoming === translation) {
      this.incoming = undefined
    }
    await this.close(translation)
    this.callbacks.onStateChange()
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
    void this.queue(() => this.showCaptions(translation.lang))
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
    for (const track of extractCatalogTracks(catalog)) {
      if (track.role === 'subtitle' && track.lang) {
        this.transcriptTracks.set(track.lang, track)
      }
    }
    if (!this.video && videoTrack) {
      this.locLive.configureTrack('video', videoTrack)
      this.video = await this.subscribe(videoTrack, (groupId, object, cached) =>
        this.locLive.push('video', groupId, object, cached)
      )
    }
    const original = this.languages.find((option) => option.lang === ORIGINAL_LANGUAGE)?.track
    if (!this.original && original) {
      this.locLive.configureTrack('audio', original)
      this.original = await this.subscribe(original, (groupId, object) => {
        this.locLive.push('audio', groupId, object)
        this.countForwarded(original.name)
      })
      await this.showCaptions(ORIGINAL_LANGUAGE)
    }
    this.callbacks.onStateChange()
  }

  /// Follows the language heard with its transcript; the source's (`text_ja`)
  /// is served only while some language is being translated, so a refusal
  /// is logged and leaves the picture without captions.
  private async showCaptions(lang: string | undefined): Promise<void> {
    if (this.captions?.lang === lang) {
      return
    }
    const previous = this.captions
    this.captions = undefined
    if (previous) {
      previous.lane.close()
      await this.unsubscribe(previous.subscribed)
    }
    const track = lang === undefined ? undefined : this.transcriptTracks.get(lang)
    if (!track) {
      return
    }
    const lane = new CaptionLane(this.locLive.playout, (text) => this.callbacks.onCaption(text))
    try {
      const subscribed = await this.subscribe(track, (_groupId, object) =>
        lane.push(object.objectPayload, readLocHeader(object.locHeader).captureTimestampMicros)
      )
      this.captions = { lang: lang!, subscribed, lane }
    } catch (error) {
      lane.close()
      this.callbacks.onLog('info', `no captions for ${lang}: ${getErrorMessage(error)}`)
    }
  }

  private countForwarded(trackName: string): void {
    this.forwardedAudioObjects[trackName] = (this.forwardedAudioObjects[trackName] ?? 0) + 1
  }

  /// Subscribes with a joining FETCH, so the relay's cache of the current group
  /// comes first and the picture and a translation start without waiting up to
  /// a group's length for the next keyframe or group start. Objects from before
  /// the live edge are flagged `cached`.
  private async subscribe(
    track: MediaCatalogTrack,
    onObject: (groupId: bigint, object: LocObject, cached: boolean) => void
  ): Promise<Subscribed> {
    const session = this.context.session
    if (!session) {
      throw new Error('the player has not started')
    }
    const namespace = catalogTrackNamespace(track, this.context.namespace)
    const handle = await session.subscribeCurrentGroup(namespace, track.name)
    void readJoinedObjects(this.context, streamId(), handle, onObject)
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
