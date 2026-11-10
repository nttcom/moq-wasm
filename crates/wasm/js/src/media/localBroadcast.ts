import type { MoqSession } from '../core/session'
import type { MediaBroadcast } from '../../pkg/moqt'
import { forwardSdkLog, sdkLog } from '../core/log'
import { audioTrackName, rejectedName, sourceTrackNames } from './sourceNames'
import { noteOwnBroadcast } from './ownBroadcasts'
import {
  type MediaKind,
  type SourceEncoding,
  type SourceOptions,
  type VideoEncodingOptions,
  checkRenditions,
  publishSource,
  videoBitrate,
  videoFramerate
} from './internal/encoders/sourceEncoding'
import type { SourcePublicationStats } from './internal/encoders/publishStats'

const TARGET = 'moq-wasm::media::local_broadcast'

export type LocalBroadcastOptions = {
  /// Lists the `chat` track in the catalog; write it with `session.publishTrack`.
  offerChat?: boolean
  /// Receives the encoders' failures. Skips a failure whose `String()` equals
  /// the previous one, such as the "media publishing stopped" every chunk
  /// fails with after publishing stops.
  onError?: (error: unknown) => void
}

/// A device the SDK captures with `getUserMedia` (`true` takes the default
/// one), or a track the page captured.
export type TrackInput = true | MediaTrackConstraints | MediaStreamTrack

export type SourceInput = Omit<SourceOptions, 'videoLabel'> & {
  /// The source's label in the catalog: the video's, and the audio's followed
  /// by ` audio`. The source name when omitted.
  label?: string
  video?: TrackInput
  audio?: TrackInput
  /// Publishes its first video and audio tracks; the SDK does not stop them.
  stream?: MediaStream
  /// The SDK captures the screen with `getDisplayMedia`: call `addSource` from
  /// a user gesture. Takes the place of `video`, `audio` and `stream`.
  display?: true | DisplayMediaStreamOptions
}

export type LocalSourceListener = {
  /// The device of `kind` the SDK captured was unplugged or its capture
  /// stopped; the source no longer publishes it. A track the page gave is the
  /// page's to watch and replace.
  onTrackEnded?(kind: MediaKind): void
  /// Every track of the source ended, the last one a device the SDK captured,
  /// such as a screen after the browser's "Stop sharing"; the source was
  /// removed from the broadcast before this call.
  onEnded?(): void
}

export interface LocalSource {
  readonly name: string
  /// The tracks the source publishes, for the page's own preview. A track
  /// replaced by `setVideo` or `setAudio` is replaced in this same stream.
  readonly stream: MediaStream
  /// Replaces the video: a new device (the SDK stops the track it captured
  /// before) or a track of the page. `false` stops publishing the video and
  /// releases a device the SDK captured. Rejects with the `getUserMedia`
  /// error, such as a `NotAllowedError`.
  setVideo(input: TrackInput | false): Promise<void>
  /// `setVideo` for the audio.
  setAudio(input: TrackInput | false): Promise<void>
  /// Stops publishing `kind` and keeps its device. Subscribers see the same as
  /// after `setVideo(false)`: the track leaves the catalog.
  setMuted(kind: MediaKind, muted: boolean): Promise<void>
  isMuted(kind: MediaKind): boolean
  /// Restarts the video's and the renditions' encoders with `options`, which
  /// replace the source's `encoding` and `main`. A muted video takes them when
  /// unmuted. The catalog keeps the bitrate and frame rate the source was
  /// published with.
  /// Does nothing once the source is closed.
  setVideoEncoding(options: VideoEncodingOptions): void
  /// What the source's encoders output over the last second, and the frames
  /// they dropped and their failures since it was published.
  stats(): SourcePublicationStats
  /// Returns the function that removes the listener.
  listen(listener: LocalSourceListener): () => void
}

export interface LocalBroadcast {
  readonly namespace: string[]
  /// Publishes the source `name` in the tracks `name`, `name_audio` and
  /// `name_<rendition>`. Rejects for a name the broadcast cannot take (empty,
  /// `catalog`, `chat`, ending in `_audio`, `_cmaf` or `_timeline`, or equal
  /// to a track of another source), once the broadcast is closed, and with
  /// the capture's error.
  addSource(name: string, input?: SourceInput): Promise<LocalSource>
  source(name: string): LocalSource | undefined
  /// Ends the source's tracks with PUBLISH_DONE and stops the devices the SDK captured.
  removeSource(name: string): Promise<void>
  /// `removeSource` for every source. The namespace stays announced until the
  /// session closes.
  close(): Promise<void>
}

/// Announces `namespace` on `session` at once and returns the broadcast its
/// sources are added to. Every track of every source is listed in one MSF
/// catalog (draft-ietf-moq-msf-01 §5).
export async function createBroadcast(
  session: MoqSession,
  namespace: string[],
  options: LocalBroadcastOptions = {}
): Promise<LocalBroadcast> {
  const broadcast = session.broadcast(namespace)
  await broadcast.announce()
  noteOwnBroadcast(session, namespace)
  if (options.offerChat) {
    await broadcast.offerChat()
  }
  return new Broadcast(session, namespace, broadcast, reporter(session, options.onError))
}

function reporter(session: MoqSession, onError: ((error: unknown) => void) | undefined): (error: unknown) => void {
  let lastReported: string | undefined
  return (error) => {
    const message = String(error)
    if (message === lastReported) {
      return
    }
    lastReported = message
    if (onError) {
      forwardSdkLog(session, 'error', TARGET, 'publish failed', { error })
      onError(error)
    } else {
      sdkLog(session, 'error', TARGET, 'publish failed', { error })
    }
  }
}

class Broadcast implements LocalBroadcast {
  private readonly sources = new Map<string, Source>()
  /// Names taken by a source still being captured, so two concurrent
  /// `addSource` calls cannot take the same one.
  private readonly pending = new Set<string>()
  /// A new encoder worker stamps its first chunk 0; every source places its
  /// chunks from here, so no source's timestamps run backwards.
  private readonly startedAtMs = performance.now()
  private closed = false

  constructor(
    private readonly session: MoqSession,
    readonly namespace: string[],
    private readonly broadcast: MediaBroadcast,
    private readonly report: (error: unknown) => void
  ) {}

  async addSource(name: string, input: SourceInput = {}): Promise<LocalSource> {
    if (this.closed) {
      throw new Error('the broadcast is closed')
    }
    checkRenditions(input.renditions)
    const trackNames = sourceTrackNames(
      name,
      (input.renditions ?? []).map((rendition) => rendition.name)
    )
    this.checkName(name, trackNames)
    this.pending.add(name)
    try {
      const tracks = await capture(input)
      try {
        const source = await this.publish(name, trackNames, input, tracks)
        this.sources.set(name, source)
        return source
      } catch (error) {
        release(tracks)
        throw error
      }
    } finally {
      this.pending.delete(name)
    }
  }

  private async publish(
    name: string,
    trackNames: string[],
    input: SourceInput,
    tracks: CapturedTrack[]
  ): Promise<Source> {
    if (this.closed) {
      throw new Error('the broadcast is closed')
    }
    const audioName = audioTrackName(name)
    const { label = name, video: _video, audio: _audio, stream: _stream, display: _display, ...encoderOptions } = input
    const options: SourceOptions = { ...encoderOptions, videoLabel: label }
    // The catalog lists a track only at its first frame; a subscriber that
    // knows the track's name may subscribe before that.
    await this.broadcast.declareTracks([name, audioName])
    const encoding = publishSource(
      (onKeyframeRequest) =>
        this.broadcast.addSource(
          name,
          label,
          videoBitrate(options),
          videoFramerate(options),
          audioName,
          onKeyframeRequest
        ),
      new MediaStream(tracks.map(({ track }) => track)),
      options,
      this.startedAtMs,
      this.report,
      this.session
    )
    return new Source(name, trackNames, encoding, tracks, () => this.removeSource(name))
  }

  source(name: string): LocalSource | undefined {
    return this.sources.get(name)
  }

  async removeSource(name: string): Promise<void> {
    const source = this.sources.get(name)
    this.sources.delete(name)
    await source?.close()
  }

  async close(): Promise<void> {
    this.closed = true
    const sources = [...this.sources.values()]
    this.sources.clear()
    await Promise.all(sources.map((source) => source.close()))
  }

  private checkName(name: string, trackNames: string[]): void {
    const reason = rejectedName(name, trackNames, this.takenNames())
    if (reason) {
      throw new Error(`a source cannot be named ${JSON.stringify(name)}: ${reason}`)
    }
  }

  private takenNames(): Set<string> {
    const names = new Set<string>(this.pending)
    for (const source of this.sources.values()) {
      for (const track of source.trackNames) {
        names.add(track)
      }
    }
    return names
  }
}

type CapturedTrack = {
  kind: MediaKind
  track: MediaStreamTrack
  /// The SDK captured it, so the SDK stops it.
  owned: boolean
}

async function capture(input: SourceInput): Promise<CapturedTrack[]> {
  if (input.display !== undefined) {
    if (input.video !== undefined || input.audio !== undefined || input.stream !== undefined) {
      throw new Error('display takes the place of video, audio and stream')
    }
    const options = input.display === true ? { video: true } : input.display
    return owned(await navigator.mediaDevices.getDisplayMedia(options))
  }
  if (input.stream !== undefined) {
    if (input.video !== undefined || input.audio !== undefined) {
      throw new Error('stream takes the place of video and audio')
    }
    const video = input.stream.getVideoTracks()[0]
    const audio = input.stream.getAudioTracks()[0]
    return [
      ...(video ? [{ kind: 'video' as const, track: video, owned: false }] : []),
      ...(audio ? [{ kind: 'audio' as const, track: audio, owned: false }] : [])
    ]
  }
  return captureInputs({ video: input.video, audio: input.audio })
}

function release(tracks: CapturedTrack[]): void {
  for (const { track, owned } of tracks) {
    if (owned) {
      track.stop()
    }
  }
}

function owned(stream: MediaStream): CapturedTrack[] {
  return [
    ...stream
      .getVideoTracks()
      .slice(0, 1)
      .map((track) => ({ kind: 'video' as const, track, owned: true })),
    ...stream
      .getAudioTracks()
      .slice(0, 1)
      .map((track) => ({ kind: 'audio' as const, track, owned: true }))
  ]
}

/// Captures every device input in one `getUserMedia` call, so the browser
/// asks for its permissions once.
async function captureInputs(inputs: Partial<Record<MediaKind, TrackInput>>): Promise<CapturedTrack[]> {
  const given: CapturedTrack[] = []
  const constraints: MediaStreamConstraints = {}
  for (const kind of ['video', 'audio'] as const) {
    const input = inputs[kind]
    if (input instanceof MediaStreamTrack) {
      given.push({ kind, track: input, owned: false })
    } else if (input !== undefined) {
      constraints[kind] = input
    }
  }
  if (constraints.video === undefined && constraints.audio === undefined) {
    return given
  }
  return [...given, ...owned(await navigator.mediaDevices.getUserMedia(constraints))]
}

class Source implements LocalSource {
  readonly stream = new MediaStream()
  private readonly tracks = new Map<MediaKind, CapturedTrack>()
  private readonly listeners = new Set<LocalSourceListener>()
  private closed = false

  constructor(
    readonly name: string,
    readonly trackNames: string[],
    private readonly encoding: SourceEncoding,
    tracks: CapturedTrack[],
    private readonly removeFromBroadcast: () => Promise<void>
  ) {
    for (const track of tracks) {
      this.attach(track)
    }
  }

  setVideo(input: TrackInput | false): Promise<void> {
    return this.set('video', input)
  }

  setAudio(input: TrackInput | false): Promise<void> {
    return this.set('audio', input)
  }

  setMuted(kind: MediaKind, muted: boolean): Promise<void> {
    return this.encoding.setMuted(kind, muted)
  }

  isMuted(kind: MediaKind): boolean {
    return this.encoding.isMuted(kind)
  }

  setVideoEncoding(options: VideoEncodingOptions): void {
    this.encoding.setVideoEncoding(options)
  }

  stats(): SourcePublicationStats {
    return this.encoding.stats()
  }

  listen(listener: LocalSourceListener): () => void {
    this.listeners.add(listener)
    return () => this.listeners.delete(listener)
  }

  async close(): Promise<void> {
    this.closed = true
    await this.encoding.close()
    for (const kind of [...this.tracks.keys()]) {
      this.detach(kind)
    }
  }

  private async set(kind: MediaKind, input: TrackInput | false): Promise<void> {
    if (this.closed) {
      return
    }
    if (input === false) {
      await this.encoding.removeTrack(kind)
      this.detach(kind)
      return
    }
    const [next] = await captureInputs({ [kind]: input })
    if (this.closed) {
      release([next])
      return
    }
    // The encoder moves to the new track before the old one is stopped.
    this.encoding.replaceTrack(kind, next.track)
    this.detach(kind)
    this.attach(next)
  }

  private attach(captured: CapturedTrack): void {
    this.tracks.set(captured.kind, captured)
    this.stream.addTrack(captured.track)
    if (captured.owned) {
      captured.track.addEventListener('ended', () => void this.trackEnded(captured), { once: true })
    }
  }

  private detach(kind: MediaKind): void {
    const captured = this.tracks.get(kind)
    if (!captured) {
      return
    }
    this.tracks.delete(kind)
    this.stream.removeTrack(captured.track)
    release([captured])
  }

  private async trackEnded(captured: CapturedTrack): Promise<void> {
    if (this.closed || this.tracks.get(captured.kind) !== captured) {
      return
    }
    await this.encoding.removeTrack(captured.kind, captured.track)
    if (this.tracks.get(captured.kind) !== captured) {
      return
    }
    this.detach(captured.kind)
    for (const listener of this.listeners) {
      listener.onTrackEnded?.(captured.kind)
    }
    if (this.tracks.size > 0) {
      return
    }
    await this.removeFromBroadcast()
    for (const listener of this.listeners) {
      listener.onEnded?.()
    }
  }
}
