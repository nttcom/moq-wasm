import type { MoqtClientWrapper } from '@moqt/moqtClient'
import { parse_msf_catalog_json } from '../../pkg/moqt_client_wasm'
import {
  type MediaCatalogTrack,
  extractCatalogAudioTracks,
  extractCatalogCmafTracks,
  extractCatalogMediaTimelineTracks,
  extractCatalogVideoTracks
} from '../../examples/media/catalog'
import { type StatusState, getErrorMessage } from '../../examples/media/common'
import { base64ToUint8Array } from '../../utils/media/base64'
import { parseAudioChannelCount } from '../../utils/media/decoderCatalog'
import type { MseTrackSource } from '../../utils/media/mseSink'
import type { BufferPolicy } from './jitterBuffer'
import type { CatchUp } from './livePlayout'
import type { LivePictureKind } from './livePictureSink'
import { CatalogFollower } from './catalogFollower'
import { type DeliveryObserver, UNOBSERVED_DELIVERIES } from './deliveryObserver'
import { CmafLive, cmafSource } from './cmafLive'
import { CmafReview } from './cmafReview'
import { LocLive } from './locLive'
import { LocReview } from './locReview'
import { PictureStage } from './pictureStage'
import { type ReviewFetchWindow, type ReviewHost, ReviewSession, type ReviewWindow } from './reviewSession'
import { type SeekAxis, SeekTimeline } from './seekTimeline'
import { StallWatch } from './stallWatch'
import { TextTracks } from './textTrack'
import type { LogLevel, MediaKind, Packaging, TrackContext } from './trackContext'
import type { SubgroupObjectMessageWithLoc } from '@moqt/subscriptionStateManager'
import { TrackSubscriptions } from './trackSubscriptions'

const MICROS_PER_SECOND = 1_000_000
const REVIEW_PLAYHEAD_STEP_US = 1_000_000

export type { Packaging } from './trackContext'

export type LivePlayerCallbacks = {
  onStateChange(): void
  onLiveFrame(): void
  onLog(level: LogLevel, message: string): void
}

export type LivePlayerOptions = {
  client: MoqtClientWrapper
  container: HTMLElement
  callbacks: LivePlayerCallbacks
  deliveryObserver?: DeliveryObserver
  livePicture?: LivePictureKind
}

export type PlayerStatus = { text: string; state: StatusState }

export type LivePlayerState = {
  started: boolean
  videoTracks: MediaCatalogTrack[]
  audioTracks: MediaCatalogTrack[]
  selectedVideoTrack: string
  selectedAudioTrack: string
  cmafAvailable: boolean
  packaging: Packaging
  mode: 'live' | 'review'
  paused: boolean
  stalled: boolean
  playbackRate: number
  playbackRateAdjustable: boolean
  catalogStatus: PlayerStatus
  playbackStatus: PlayerStatus
  rewindStatus: PlayerStatus
  seek: SeekAxis & { anchorSeconds: number | undefined; playheadSeconds: number | undefined }
  delivery: {
    liveVideoAlias: bigint | undefined
    liveAudioAlias: bigint | undefined
    mediaTimelineTrackName: string | undefined
    reviewFetchWindows: ReviewFetchWindow[]
  }
}

export type LivePlayerStats = {
  frameSize: { width: number; height: number } | undefined
  viewerDelayMs: number | undefined
  bufferMs: number | undefined
  targetBufferMs: number
  fixedBuffer: boolean
  outputLatencyMs: number
  arrivalSpreadMs: number | undefined
  receivedKbps: number
  videoObjects: number
  syncOffsetMs: number | undefined
  audioBreaks: number
  videoDrops: string
  shedMs: number
}

export class LivePlayer {
  private readonly context: TrackContext
  private readonly callbacks: LivePlayerCallbacks
  private readonly textTracks: TextTracks
  private readonly catalog: CatalogFollower
  private readonly subscriptions: TrackSubscriptions
  private readonly timeline = new SeekTimeline()
  private readonly stage: PictureStage
  private readonly locLive: LocLive
  private readonly cmafLive = new CmafLive()
  private readonly locReview: LocReview
  private readonly cmafReview = new CmafReview()
  private readonly stallWatch: StallWatch
  private readonly reviewHost: ReviewHost
  private tracks: Record<MediaKind, MediaCatalogTrack[]> = { video: [], audio: [] }
  private cmafTracks: MediaCatalogTrack[] = []
  private selected: Record<MediaKind, string> = { video: '', audio: '' }
  private packaging: Packaging = 'loc'
  private review: ReviewSession | undefined
  private started = false
  private trackChanges: Promise<void> = Promise.resolve()
  private paused = false
  private volume = 1
  private playbackRate = 1
  private videoObjectCount = 0
  private catalogStatus: PlayerStatus = { text: 'Catalog not loaded yet', state: 'idle' }
  private playbackStatus: PlayerStatus = { text: 'Playback idle', state: 'idle' }
  private rewindStatus: PlayerStatus = { text: 'Live', state: 'ok' }

  constructor(options: LivePlayerOptions) {
    this.callbacks = options.callbacks
    this.context = {
      client: options.client,
      namespace: [],
      authInfo: '',
      observer: options.deliveryObserver ?? UNOBSERVED_DELIVERIES,
      log: (level, message) => options.callbacks.onLog(level, message)
    }
    this.textTracks = new TextTracks(this.context)
    this.catalog = new CatalogFollower(
      this.context,
      this.textTracks,
      (text) => void this.queueTrackChange(() => this.applyCatalog(text))
    )
    this.subscriptions = new TrackSubscriptions(this.context)
    this.stallWatch = new StallWatch(() => this.changed())
    this.stage = new PictureStage(options.container)
    this.locLive = new LocLive(this.stage.liveVideo, this.stage.liveCanvas, options.livePicture, {
      onPresented: (picture) => this.notePresentedFrame(picture),
      onFrameShown: (ids, captureMicros) => {
        const trackAlias = this.subscriptions.get('video')?.trackAlias
        if (ids && trackAlias !== undefined) {
          this.context.observer.setPlayhead({ kind: 'subscribe', trackAlias, ...ids, captureMicros })
        }
        this.callbacks.onLiveFrame()
      }
    })
    this.locReview = new LocReview(this.stage.reviewCanvas, {
      onFrameShown: (captureMicros, playhead) => {
        if (playhead) {
          this.context.observer.setPlayhead(playhead)
        }
        this.stage.show(this.locReview.canvas)
        this.notePresentedFrame(this.locReview.canvas)
        this.advanceReviewPlayhead(captureMicros)
      },
      onLog: this.context.log
    })
    this.reviewHost = {
      playWindow: (window, session) => this.playReviewWindow(window, session),
      windowStarted: () => this.renderReviewStatus(),
      fastForwardDrained: () => this.playbackRateAdjustable() && this.playbackRate > 1 && this.cmafReview.drained(),
      observeLiveVideoGroup: (groupId) => this.observeLiveVideoGroup(groupId),
      resumeLiveForward: () => this.resumeLiveForward(),
      goLive: () => this.goLive(),
      seek: (captureMicros) => this.seek(captureMicros),
      failed: (description) => {
        this.stallWatch.clear()
        this.setRewindStatus(`Rewind failed: ${description}`, 'error')
      }
    }
    this.stage.show(this.locLive.picture.element)
    for (const video of this.stage.videos()) {
      this.watchPresentedFrames(video)
    }
  }

  get state(): LivePlayerState {
    const axis = this.timeline.axis()
    const review = this.review
    return {
      started: this.started,
      videoTracks: this.tracks.video,
      audioTracks: this.tracks.audio,
      selectedVideoTrack: this.selected.video,
      selectedAudioTrack: this.selected.audio,
      cmafAvailable: this.cmafTracks.length > 0,
      packaging: this.packaging,
      mode: review ? 'review' : 'live',
      paused: this.paused,
      stalled: this.stallWatch.stalled,
      playbackRate: this.playbackRate,
      playbackRateAdjustable: this.playbackRateAdjustable(),
      catalogStatus: this.catalogStatus,
      playbackStatus: this.playbackStatus,
      rewindStatus: this.rewindStatus,
      seek: {
        ...axis,
        anchorSeconds: review && review.anchorMicros / MICROS_PER_SECOND,
        playheadSeconds: review && review.playheadMicros / MICROS_PER_SECOND
      },
      delivery: {
        liveVideoAlias: this.subscriptions.get('video')?.trackAlias,
        liveAudioAlias: this.subscriptions.get('audio')?.trackAlias,
        mediaTimelineTrackName: this.timeline.mediaTimelineTrackName,
        reviewFetchWindows: review?.fetchWindows ?? []
      }
    }
  }

  stats(): LivePlayerStats {
    const playout = this.locLive.playout
    return {
      frameSize: this.locLive.frameSize,
      viewerDelayMs: this.locLive.viewerDelayMs,
      bufferMs: playout.bufferMs(),
      targetBufferMs: playout.targetBufferMs(),
      fixedBuffer: playout.fixedBuffer(),
      outputLatencyMs: playout.outputLatencyMs(),
      arrivalSpreadMs: playout.arrivalSpreadMs(),
      receivedKbps: this.locLive.receivedKbps,
      videoObjects: this.videoObjectCount,
      syncOffsetMs: playout.syncOffsetMs(),
      audioBreaks: playout.audioBreaks(),
      videoDrops: playout.videoDrops(),
      shedMs: playout.shedMs()
    }
  }

  elapsedMsAt(captureMicros: number): number | undefined {
    return this.timeline.media.elapsedMsAt(captureMicros)
  }

  async start(namespace: string[], authInfo: string): Promise<void> {
    await this.queueTrackChange(async () => {
      this.context.namespace = namespace
      this.context.authInfo = authInfo
      this.started = true
      this.changed()
    })
    await this.catalog.follow()
  }

  stop(): Promise<void> {
    return this.queueTrackChange(() => this.stopWatching())
  }

  private async stopWatching(): Promise<void> {
    this.started = false
    this.timeline.reset()
    this.goLive()
    this.cmafLive.close()
    this.locLive.reset()
    this.stallWatch.clear()
    this.stage.show(this.locLive.picture.element)
    for (const [kind] of this.subscriptions.entries()) {
      await this.subscriptions.unsubscribe(kind)
    }
    await this.textTracks.unsubscribeAll()
    this.catalog.reset()
    this.tracks = { video: [], audio: [] }
    this.cmafTracks = []
    this.selected = { video: '', audio: '' }
    this.videoObjectCount = 0
    this.catalogStatus = { text: 'Catalog not loaded yet', state: 'idle' }
    this.playbackStatus = { text: 'Playback idle', state: 'idle' }
    this.changed()
  }

  selectVideoTrack(name: string): Promise<void> {
    return this.selectTrack('video', name)
  }

  selectAudioTrack(name: string): Promise<void> {
    return this.selectTrack('audio', name)
  }

  private selectTrack(kind: MediaKind, name: string): Promise<void> {
    return this.queueTrackChange(async () => {
      this.selected[kind] = name
      await this.resubscribe(kind)
      await this.openLiveMse()
    })
  }

  setPackaging(packaging: Packaging): Promise<void> {
    return this.queueTrackChange(() => this.switchPackaging(packaging))
  }

  private async switchPackaging(packaging: Packaging): Promise<void> {
    if (packaging === this.packaging) {
      return
    }
    this.packaging = packaging
    this.changed()
    await this.resubscribe('video')
    await this.resubscribe('audio')
    if (this.packaging === 'cmaf') {
      await this.openLiveMse()
    } else {
      const previous = this.cmafLive.detach()
      this.stage.replace(this.locLive.picture.element, () => this.packaging === 'loc' && !this.review, previous)
    }
    this.context.log('info', `packaging switched to ${this.packaging}`)
  }

  setBufferPolicy(policy: BufferPolicy): void {
    this.locLive.setBufferPolicy(policy)
  }

  setCatchUp(catchUp: CatchUp): void {
    this.locLive.playout.setCatchUp(catchUp)
  }

  skip(seconds: number): void {
    const latest = this.timeline.groups.latest
    if (!latest) {
      return
    }
    this.seek((this.review?.playheadMicros ?? latest.captureMicros) + seconds * MICROS_PER_SECOND)
  }

  /// A position at or past the live edge goes live. Any other position is
  /// replayed from the closed keyframe group that holds it: the frames before it
  /// are decoded without pacing and only the ones from the position on are shown.
  seek(captureMicros: number): void {
    const latest = this.timeline.groups.latest
    if (!latest) {
      return
    }
    if (captureMicros >= latest.captureMicros) {
      this.goLive()
      return
    }
    const target = this.timeline.groups.resolveSeekTarget(captureMicros)
    if (!target) {
      this.setRewindStatus('Rewind unavailable: nothing buffered yet', 'error')
      return
    }

    this.review?.end()
    const session = new ReviewSession(
      this.context,
      this.timeline,
      this.subscriptions,
      this.reviewHost,
      target,
      captureMicros
    )
    this.review = session
    this.subscriptions.pauseForward()
    this.cmafReview.needsOpen = true
    this.setPaused(false)
    this.locReview.playout.start(session.anchorMicros)
    this.applyVolume()
    this.playbackStatus = { text: 'Reviewing', state: 'review' }
    this.changed()
    session.start()
  }

  goLive(): void {
    this.review?.end()
    this.review = undefined
    this.resumeLiveForward()
    this.locReview.clearFrameIds()
    this.context.observer.clearPlayhead('fetch')
    this.setPaused(false)
    this.locReview.playout.stop()
    this.playbackRate = 1
    this.applyVolume()
    this.stage.show(this.livePicture())
    this.cmafReview.close()
    this.rewindStatus = { text: 'Live', state: 'ok' }
    this.changed()
  }

  /// Pausing holds whatever is on screen; every other transition (seek, skip,
  /// live, packaging or quality change) resumes. Resuming live CMAF jumps to the
  /// end of what is buffered so the picture is live again; the LOC MediaStream
  /// has no backlog to skip.
  setPaused(paused: boolean): void {
    this.paused = paused
    if (paused) {
      this.stallWatch.clear()
    }
    for (const media of this.playingMedia()) {
      if (paused) {
        media.pause()
      } else {
        void media.play().catch(() => undefined)
      }
    }
    const liveSink = this.cmafLive.sink
    if (!this.review && !liveSink) {
      this.locLive.playout.setPaused(paused)
    }
    if (this.review && this.packaging === 'loc') {
      this.locReview.playout.setPaused(paused)
    }
    const liveEnd = liveSink?.bufferedEnd()
    if (!paused && !this.review && liveSink && liveEnd !== undefined) {
      liveSink.element.currentTime = liveEnd
    }
    this.changed()
  }

  /// Review carries its own sound, so whatever live audio is still buffered when
  /// a review starts is silenced, and it is heard again on the way back to live.
  setVolume(volume: number): void {
    this.volume = volume
    this.applyVolume()
  }

  setPlaybackRate(rate: number): void {
    this.playbackRate = rate
    this.changed()
  }

  /// A change unsubscribes and resubscribes media tracks across awaits, so two
  /// running at once would both subscribe the same kind and leave one of the
  /// subscriptions behind; changes run one after another in the order they came.
  private queueTrackChange(change: () => Promise<void>): Promise<void> {
    const run = this.trackChanges.then(change)
    this.trackChanges = run.catch(() => undefined)
    return run
  }

  private changed(): void {
    this.applyPlaybackRate()
    this.callbacks.onStateChange()
  }

  private setRewindStatus(text: string, state: StatusState): void {
    this.rewindStatus = { text, state }
    this.changed()
  }

  private async applyCatalog(payload: string): Promise<void> {
    if (!this.started) {
      return
    }
    try {
      const catalog = parse_msf_catalog_json(payload)
      const videoChanged = this.replaceTracks('video', extractCatalogVideoTracks(catalog).filter(isLocTrack))
      const audioChanged = this.replaceTracks('audio', extractCatalogAudioTracks(catalog).filter(isLocTrack))
      this.cmafTracks = extractCatalogCmafTracks(catalog)
      this.catalogStatus = {
        text: `Catalog loaded: ${this.tracks.video.length} video / ${this.tracks.audio.length} audio`,
        state: 'ok'
      }
      this.changed()
      await this.subscribeMediaTimeline(catalog)
      if (videoChanged || audioChanged) {
        await this.resubscribe('video')
        await this.resubscribe('audio')
        await this.openLiveMse()
      } else {
        this.reconfigureDecoders()
      }
    } catch (error) {
      this.catalogStatus = { text: `Catalog error: ${getErrorMessage(error)}`, state: 'error' }
      this.changed()
      this.context.log('error', `catalog: ${getErrorMessage(error)}`)
    }
  }

  private replaceTracks(kind: MediaKind, tracks: MediaCatalogTrack[]): boolean {
    const previous = this.tracks[kind].map((track) => track.name)
    this.tracks[kind] = tracks
    const names = tracks.map((track) => track.name)
    if (names.length === previous.length && names.every((name, index) => name === previous[index])) {
      return false
    }
    this.selected[kind] = names.includes(this.selected[kind]) ? this.selected[kind] : (names[0] ?? '')
    return true
  }

  private async subscribeMediaTimeline(catalog: unknown): Promise<void> {
    const [track] = extractCatalogMediaTimelineTracks(catalog)
    if (!track || !this.timeline.followMediaTimeline(track)) {
      return
    }
    await this.textTracks.subscribe(track.name, (text) => {
      try {
        this.timeline.replaceMediaTimeline(text)
      } catch (error) {
        this.context.log('error', `media timeline: ${getErrorMessage(error)}`)
        return
      }
      this.changed()
    })
  }

  private cmafSibling(track: MediaCatalogTrack): MediaCatalogTrack | undefined {
    return this.cmafTracks.find((candidate) => candidate.name === `${track.name}_cmaf`)
  }

  private async resubscribe(kind: MediaKind): Promise<void> {
    const trackName = this.selected[kind]
    const track = this.tracks[kind].find((candidate) => candidate.name === trackName)
    const wire = track && (this.packaging === 'cmaf' ? this.cmafSibling(track) : track)
    if (wire && this.subscriptions.get(kind)?.name === wire.name) {
      return
    }

    if (kind === 'video') {
      this.timeline.resetGroups()
      this.goLive()
    } else {
      this.timeline.audio.reset()
    }
    await this.subscriptions.unsubscribe(kind)
    if (!track || !wire) {
      return
    }

    if (this.packaging === 'cmaf') {
      await this.subscriptions.subscribe(kind, wire.name, track, (groupId, object) =>
        this.handleCmafObject(kind, wire.name, groupId, object)
      )
      return
    }
    this.locLive.configureTrack(kind, track)
    await this.subscriptions.subscribe(kind, wire.name, track, (groupId, object) =>
      this.handleLocObject(kind, trackName, groupId, object)
    )
  }

  private handleLocObject(
    kind: MediaKind,
    trackName: string,
    groupId: bigint,
    object: SubgroupObjectMessageWithLoc
  ): void {
    if (kind === 'video') {
      this.videoObjectCount += 1
      this.timeline.groups.record(groupId, object.locHeader)
      this.notePlaying(trackName)
    } else {
      this.timeline.audio.record(groupId, object.locHeader)
    }
    this.locLive.push(kind, groupId, object)
  }

  private handleCmafObject(
    kind: MediaKind,
    trackName: string,
    groupId: bigint,
    object: SubgroupObjectMessageWithLoc
  ): void {
    if (object.objectStatus != null) {
      return
    }
    if (kind === 'video') {
      this.videoObjectCount += 1
      if (object.objectId === 0n) {
        this.timeline.observeTimelineGroup(groupId)
      }
      this.notePlaying(trackName)
    } else {
      this.timeline.audio.record(groupId, undefined)
    }
    this.cmafLive.append(kind, object)
  }

  private notePlaying(trackName: string): void {
    if (!this.review) {
      this.playbackStatus = { text: `Playing ${trackName}`, state: 'ok' }
    }
    this.changed()
  }

  /// A catalog update may redefine a track under the same name, as when the
  /// publisher is replaced by one with another audio codec, so the decoders
  /// take the new definition of the tracks they are already subscribed to.
  private reconfigureDecoders(): void {
    if (this.packaging !== 'loc') {
      return
    }
    for (const [kind, subscription] of this.subscriptions.entries()) {
      const track = this.tracks[kind].find((candidate) => candidate.name === subscription.track.name)
      if (!track || JSON.stringify(track) === JSON.stringify(subscription.track)) {
        continue
      }
      this.locLive.configureTrack(kind, track)
      subscription.track = track
      this.context.log('info', `${kind} track ${track.name} redefined by the catalog`)
    }
  }

  private subscribedCmafSource(kind: MediaKind): MseTrackSource | undefined {
    const name = this.subscriptions.get(kind)?.name
    const track = this.cmafTracks.find((candidate) => candidate.name === name)
    return track && cmafSource(track)
  }

  private async openLiveMse(): Promise<void> {
    if (this.packaging !== 'cmaf') {
      return
    }
    const video = this.subscribedCmafSource('video')
    if (!video) {
      return
    }
    const previous = await this.cmafLive.open(this.stage.freeMseElement(), {
      video,
      audio: this.subscribedCmafSource('audio')
    })
    const next = this.cmafLive.sink
    this.applyVolume()
    if (next) {
      this.stage.replace(next.element, () => this.cmafLive.sink === next && !this.review, previous)
    }
  }

  private livePicture(): HTMLElement {
    return this.cmafLive.sink?.element ?? this.locLive.picture.element
  }

  private wantedPicture(): HTMLElement | undefined {
    if (!this.review) {
      return this.livePicture()
    }
    return this.packaging === 'cmaf' ? this.cmafReview.sink?.element : this.locReview.canvas
  }

  private watchPresentedFrames(video: HTMLVideoElement): void {
    const onFrame = () => {
      this.notePresentedFrame(video)
      video.requestVideoFrameCallback(onFrame)
    }
    video.requestVideoFrameCallback(onFrame)
  }

  /// A picture being replaced plays on until its successor has presented a
  /// frame, and the live picture plays out what it had buffered behind a review;
  /// only the picture playback is trying to show counts as progress.
  private notePresentedFrame(picture: HTMLElement): void {
    if (this.paused || picture !== this.wantedPicture()) {
      return
    }
    this.stallWatch.framePresented()
  }

  private resumeLiveForward(): void {
    if (this.subscriptions.resumeForward()) {
      this.cmafLive.sink?.resumeAtNewestRange()
    }
  }

  /// The media timeline records the groups of the tracks it depends on. A group
  /// of any other rendition is placed from the arrival lag of the last live
  /// object, which runs late by however long the main thread took to handle the
  /// TRACK_STATUS answer.
  private observeLiveVideoGroup(groupId: bigint): void {
    const name = this.subscriptions.get('video')?.name
    if (this.packaging === 'cmaf' || this.timeline.stampedByMediaTimeline(name)) {
      this.timeline.observeTimelineGroup(groupId)
    } else {
      this.timeline.groups.recordLiveGroup(groupId)
    }
    this.changed()
  }

  private async playReviewWindow(window: ReviewWindow, session: ReviewSession): Promise<boolean> {
    if (this.packaging === 'cmaf') {
      return this.playCmafReviewWindow(window, session)
    }
    const config = this.reviewVideoConfig()
    if (!config) {
      this.setRewindStatus('Rewind unavailable: the video track has no codec', 'error')
      return false
    }
    return this.locReview.play(
      window.frames,
      window.audio,
      config,
      this.reviewAudioConfig(),
      session.originMicros,
      session.isCurrent
    )
  }

  private async playCmafReviewWindow(window: ReviewWindow, session: ReviewSession): Promise<boolean> {
    if (this.cmafReview.needsOpen) {
      const video = this.subscribedCmafSource('video')
      if (!video) {
        this.setRewindStatus('Rewind unavailable: the CMAF track has no init segment', 'error')
        return false
      }
      const origin = session.originMicros
      const previous = await this.cmafReview.open(
        this.stage.freeMseElement(),
        {
          video,
          audio: this.subscribedCmafSource('audio'),
          startAtSeconds: (session.anchorMicros - origin) / MICROS_PER_SECOND
        },
        (secondsFromStart) => {
          if (session.isCurrent()) {
            this.advanceReviewPlayhead(origin + secondsFromStart * MICROS_PER_SECOND)
          }
        }
      )
      const next = this.cmafReview.sink
      this.applyVolume()
      this.applyPlaybackRate()
      if (next) {
        this.stage.replace(next.element, session.isCurrent, previous)
      }
    }
    return this.cmafReview.append(window.frames, window.audio, session.isCurrent)
  }

  private reviewVideoConfig(): VideoDecoderConfig | undefined {
    const track = this.subscriptions.get('video')?.track
    if (!track?.codec) {
      return undefined
    }
    return { codec: track.codec, optimizeForLatency: true }
  }

  private reviewAudioConfig(): AudioDecoderConfig | undefined {
    const track = this.subscriptions.get('audio')?.track
    if (!track?.codec || !track.samplerate) {
      return undefined
    }
    return {
      codec: track.codec,
      sampleRate: track.samplerate,
      numberOfChannels: parseAudioChannelCount(track.channelConfig) ?? 2,
      description: track.initData ? base64ToUint8Array(track.initData) : undefined
    }
  }

  /// The decoder emits frames in bursts, so the readout steps a second at a time
  /// instead of following every frame. The thumb stays on the position that was
  /// seeked to and the progress fill carries the movement.
  private advanceReviewPlayhead(captureMicros: number): void {
    const review = this.review
    if (!review || Math.abs(captureMicros - review.playheadMicros) < REVIEW_PLAYHEAD_STEP_US) {
      return
    }
    review.playheadMicros = captureMicros
    this.renderReviewStatus()
  }

  /// Starting the next window's fetch can already have taken playback live, so
  /// a status for the window is only shown while still reviewing.
  private renderReviewStatus(): void {
    const review = this.review
    if (!review) {
      return
    }
    const offset = this.locReview.playout.syncOffsetMs()
    const sync = offset === undefined ? '' : ` · A/V ${formatSyncOffset(offset)}`
    this.setRewindStatus(`Rewound ${review.behindSeconds.toFixed(1)}s${sync}`, 'review')
  }

  private playingMedia(): HTMLMediaElement[] {
    if (this.review) {
      return this.cmafReview.sink ? [this.cmafReview.sink.element] : []
    }
    if (this.cmafLive.sink) {
      return [this.cmafLive.sink.element]
    }
    const live = this.locLive.picture.element
    return live instanceof HTMLMediaElement ? [live] : []
  }

  private applyVolume(): void {
    const liveVolume = this.review ? 0 : this.volume
    this.locLive.playout.setVolume(liveVolume)
    this.locReview.playout.setVolume(this.volume)
    if (this.cmafLive.sink) {
      this.cmafLive.sink.element.volume = liveVolume
    }
    if (this.cmafReview.sink) {
      this.cmafReview.sink.element.volume = this.volume
    }
  }

  /// Only review playback through MSE can run at another rate: live playback
  /// has to keep pace with the publisher, and the WebCodecs path paces frames
  /// itself. Loading a new source resets the element's rate, so the chosen speed
  /// is applied again whenever the review MediaSource is opened.
  private playbackRateAdjustable(): boolean {
    return this.packaging === 'cmaf' && this.review !== undefined
  }

  private applyPlaybackRate(): void {
    if (this.playbackRateAdjustable() && this.cmafReview.sink) {
      this.cmafReview.sink.element.playbackRate = this.playbackRate
    }
  }
}

/// The bridge lists a CMAF sibling next to every LOC track; the WebCodecs
/// decoders only take the LOC ones.
function isLocTrack(track: MediaCatalogTrack): boolean {
  return track.packaging !== 'cmaf'
}

export function formatSyncOffset(offsetMs: number | undefined): string {
  if (offsetMs === undefined) {
    return '--'
  }
  const rounded = Math.round(offsetMs)
  return `${rounded < 0 ? '-' : '+'}${Math.abs(rounded)} ms`
}
