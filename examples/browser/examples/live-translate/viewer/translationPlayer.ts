import { type MoqSession, type PublishDone, RequestError } from 'moq-wasm'
import {
  type AddedAudioTrack,
  type AddedTextTrack,
  type LivePlayer,
  type MediaCatalogTrack,
  RemoteBroadcast,
  type RemoteBroadcastOptions,
  type SourceView,
  extractCatalogTracks,
  parseCatalog
} from 'moq-wasm/media'
import { getErrorMessage } from '../../../utils/page'
import { CaptionLane } from './captionLane'
import { type LanguageOption, ORIGINAL_LANGUAGE, languageOptions } from './languages'

/// Every track plays this long after its capture timestamp, so a translation
/// that arrives a few seconds after the speech still plays with its picture.
/// It must exceed Gemini's delay (about 3.7 s); the rest absorbs the jitter.
const DEFAULT_PLAYBACK_DELAY_MS = 5_000
const CROSSFADE_TIME_CONSTANT_SECONDS = 0.08
const RETIRED_TRANSLATION_CLOSE_DELAY_MS = 500
const MONITORED_ORIGINAL_VOLUME = 0.5
const MAX_LATE_CAPTION_MS = 3_000

type Log = NonNullable<RemoteBroadcastOptions['onLog']>

type Translation = {
  lang: string
  trackName: string
  selectedAtMs: number
  audio: AddedAudioTrack | undefined
  crossfadeTimer?: ReturnType<typeof setTimeout>
  closed?: boolean
}

export type TranslationPlayerCallbacks = {
  onStateChange(): void
  onLog: Log
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

/// Plays a live translation broadcast through a `RemoteBroadcast`: the
/// source's video and original audio `playbackDelayMs` behind capture, and
/// the selected language's translation added next to the original and
/// crossfaded in once it has caught up with the clock. The original audio
/// stays subscribed, so going back to it is a crossfade too.
export class TranslationPlayer {
  private broadcast: RemoteBroadcast | undefined
  private view: SourceView | undefined
  private changes: Promise<void> = Promise.resolve()
  private translation: Translation | undefined
  private incoming: Translation | undefined
  private droppedLateTranslatedAudio = 0
  private monitorOriginal = false
  private lastSwitchSeconds: number | undefined
  private notice: string | undefined
  languages: LanguageOption[] = []
  private transcriptTracks = new Map<string, MediaCatalogTrack>()
  private captions: { lang: string; added: AddedTextTrack; lane: CaptionLane } | undefined

  readonly playbackDelayMs: number

  constructor(
    private readonly container: HTMLElement,
    private readonly callbacks: TranslationPlayerCallbacks,
    options: TranslationPlayerOptions = {}
  ) {
    this.playbackDelayMs = options.playbackDelayMs ?? DEFAULT_PLAYBACK_DELAY_MS
  }

  get watching(): boolean {
    return this.broadcast !== undefined
  }

  /// Started, but the broadcast's translated namespace is not published yet.
  get waiting(): boolean {
    return this.watching && !this.view
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
    const live = this.view?.stats()
    const translations = [this.translation, this.incoming].flatMap((translation) => translation ?? [])
    const translatedMicros = this.translation?.audio?.captureMicros
    const originalMicros = live?.audioCaptureMicros
    return {
      language: this.language,
      pendingLanguage: this.pendingLanguage,
      lastSwitchSeconds: this.lastSwitchSeconds,
      forwardedAudioObjects: Object.fromEntries([
        ...(live && this.view ? [[this.view.player.state.selectedAudioTrack, live.audioObjects]] : []),
        ...translations.map(({ trackName, audio }) => [trackName, audio?.receivedObjects ?? 0])
      ]),
      frameSize: live?.frameSize,
      receivedVideoKbps: live?.receivedKbps ?? 0,
      droppedLateAudio:
        (live?.audioDroppedLateSamples ?? 0) +
        this.droppedLateTranslatedAudio +
        translations.reduce((sum, { audio }) => sum + (audio?.droppedLate ?? 0), 0),
      droppedLateVideo: live?.videoDroppedLateFrames ?? 0,
      playingAudioMicros: this.translation ? translatedMicros : originalMicros,
      syncOffsetMs: live?.syncOffsetMs,
      translationOffsetMs:
        translatedMicros === undefined || originalMicros === undefined
          ? undefined
          : (translatedMicros - originalMicros) / 1_000,
      viewerDelayMs: live?.viewerDelayMs,
      arrivalDelayMs: live?.fastestArrivalDelayMs,
      bufferMs: live?.bufferMs
    }
  }

  /// Until the streamer starts and the translator announces the broadcast,
  /// nobody publishes the namespace and the broadcast waits for it.
  start(session: MoqSession, namespace: string[]): Promise<void> {
    return this.queue(async () => {
      // The picture is the page's only one, so it is never paused for being off screen.
      const broadcast = new RemoteBroadcast(session, namespace, {
        pauseHiddenVideo: false,
        onLog: this.callbacks.onLog
      })
      this.broadcast = broadcast
      broadcast.listen({
        onSourceAdd: (view) => void this.queue(() => this.show(view)),
        onSourceRemove: (view) => void this.queue(() => this.forget(view))
      })
      broadcast.catalog.listen({
        onCatalog: (text) => void this.queue(() => this.readTranscripts(text)),
        onPublisherAbsent: () => {}
      })
    })
  }

  stop(): Promise<void> {
    return this.queue(async () => {
      const broadcast = this.broadcast
      this.broadcast = undefined
      // Disposing of the broadcast closes what was added to its player.
      await broadcast?.dispose()
      this.reset()
      this.transcriptTracks.clear()
      this.notice = undefined
      this.callbacks.onStateChange()
    })
  }

  setMonitorOriginal(monitor: boolean): void {
    this.monitorOriginal = monitor
    if (this.translation) {
      this.player?.setVolume(this.originalVolumeUnderTranslation(), CROSSFADE_TIME_CONSTANT_SECONDS)
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
        this.player?.setVolume(1, CROSSFADE_TIME_CONSTANT_SECONDS)
        this.retire(this.translation)
        this.translation = undefined
        this.callbacks.onLog('info', `switched audio to ${lang}`)
        this.callbacks.onStateChange()
        await this.showCaptions(ORIGINAL_LANGUAGE)
        return
      }
      await this.addTranslation(option)
      this.callbacks.onStateChange()
    })
  }

  private get player(): LivePlayer | undefined {
    return this.view?.player
  }

  /// The broadcast has one source: the streamer's camera with its audio in every language.
  private async show(view: SourceView): Promise<void> {
    if (this.view || !this.broadcast) {
      return
    }
    this.view = view
    this.container.replaceChildren(view.element)
    view.player.setBufferPolicy({ minimumMs: this.playbackDelayMs, maximumMs: this.playbackDelayMs })
    view.player.setCatchUp('off')
    view.player.setLateSamples('drop')
    // A rendition change would have to hold the fixed delay across its splice.
    view.player.setAdaptiveVideo(false)
    view.listen({ onPlayerChange: () => this.readLanguages() })
    this.readLanguages()
    await this.showCaptions(ORIGINAL_LANGUAGE)
  }

  /// The publisher left, and its player closed what was added to it.
  private async forget(view: SourceView): Promise<void> {
    if (this.view !== view) {
      return
    }
    this.reset()
    this.callbacks.onStateChange()
  }

  private reset(): void {
    for (const translation of [this.translation, this.incoming]) {
      if (translation) {
        translation.closed = true
        clearTimeout(translation.crossfadeTimer)
      }
    }
    this.translation = this.incoming = undefined
    this.captions?.lane.close()
    this.captions = undefined
    this.view = undefined
    this.languages = []
  }

  /// The original is heard at its highest bitrate, the track its language option names.
  private readLanguages(): void {
    if (!this.view) {
      return
    }
    const { audioTracks, selectedAudioTrack } = this.view.player.state
    const languages = languageOptions(audioTracks)
    const original = languages.find((option) => option.lang === ORIGINAL_LANGUAGE)?.track.name
    if (original && original !== selectedAudioTrack) {
      void this.view.player.selectAudioTrack(original)
    }
    // The player reports every state change, most of which leave the languages as they are.
    const names = (options: LanguageOption[]) => options.map((option) => option.track.name).join()
    if (names(languages) !== names(this.languages)) {
      this.languages = languages
      this.callbacks.onStateChange()
    }
  }

  private async readTranscripts(text: string): Promise<void> {
    for (const track of extractCatalogTracks(parseCatalog(text))) {
      if (track.role === 'subtitle' && track.lang) {
        this.transcriptTracks.set(track.lang, track)
      }
    }
    if (this.view) {
      await this.showCaptions(this.language)
    }
  }

  private async addTranslation(option: LanguageOption): Promise<void> {
    const player = this.player
    if (!player) {
      return
    }
    const translation: Translation = {
      lang: option.lang,
      trackName: option.track.name,
      selectedAtMs: performance.now(),
      audio: undefined
    }
    this.incoming = translation
    try {
      translation.audio = await player.addAudioTrack(option.track.name, {
        onFirstScheduled: (dueMs) => {
          translation.crossfadeTimer = setTimeout(
            () => this.crossfade(translation),
            Math.max(0, dueMs - performance.now())
          )
        },
        onEnded: (done) => void this.queue(() => this.translationEnded(translation, done))
      })
    } catch (error) {
      if (this.incoming === translation) {
        this.incoming = undefined
      }
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
      this.player?.setVolume(1, CROSSFADE_TIME_CONSTANT_SECONDS)
      this.translation = undefined
      await this.showCaptions(ORIGINAL_LANGUAGE)
    }
    if (this.incoming === translation) {
      this.incoming = undefined
    }
    await this.close(translation)
    this.callbacks.onStateChange()
  }

  private crossfade(translation: Translation): void {
    translation.crossfadeTimer = undefined
    if (this.incoming !== translation) {
      return
    }
    this.player?.setVolume(this.originalVolumeUnderTranslation(), CROSSFADE_TIME_CONSTANT_SECONDS)
    translation.audio?.setVolume(1, CROSSFADE_TIME_CONSTANT_SECONDS)
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
    translation.audio?.setVolume(0, CROSSFADE_TIME_CONSTANT_SECONDS)
    setTimeout(() => void this.queue(() => this.close(translation)), RETIRED_TRANSLATION_CLOSE_DELAY_MS)
  }

  private async close(translation: Translation | undefined): Promise<void> {
    if (!translation || translation.closed) {
      return
    }
    translation.closed = true
    clearTimeout(translation.crossfadeTimer)
    this.droppedLateTranslatedAudio += translation.audio?.droppedLate ?? 0
    await translation.audio?.close()
    this.callbacks.onLog('info', `unsubscribed ${translation.trackName}`)
  }

  private queue(change: () => Promise<void>): Promise<void> {
    const run = this.changes.then(change)
    this.changes = run.catch((error) => this.callbacks.onLog('error', getErrorMessage(error)))
    return run
  }

  /// Follows the language heard with its transcript; the source's (`text_ja`)
  /// is served only while some language is being translated, so a refusal
  /// is logged and leaves the picture without captions.
  private async showCaptions(lang: string): Promise<void> {
    if (this.captions?.lang === lang) {
      return
    }
    const previous = this.captions
    this.captions = undefined
    if (previous) {
      previous.lane.close()
      await previous.added.close()
    }
    const track = this.transcriptTracks.get(lang)
    const player = this.player
    if (!track || !player) {
      return
    }
    const lane = new CaptionLane((text) => this.callbacks.onCaption(text))
    try {
      const added = await player.addTextTrack(track.name, {
        onText: (text, lateMs) => {
          if (lateMs <= MAX_LATE_CAPTION_MS) {
            lane.push(text)
          }
        }
      })
      this.captions = { lang, added, lane }
    } catch (error) {
      lane.close()
      this.callbacks.onLog('info', `no captions for ${lang}: ${getErrorMessage(error)}`)
    }
  }
}
