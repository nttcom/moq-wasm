import { getErrorMessage } from '../../examples/media/common'
import { type GroupMark, type ReviewFrame, sortReviewFrames } from './rewind'
import { type FetchFailure, fetchFrames, isFetchFailure } from './reviewFetch'
import type { GroupRange } from './audioGroups'
import type { SeekTimeline } from './seekTimeline'
import type { MediaKind, TrackContext } from './trackContext'
import type { TrackSubscriptions } from './trackSubscriptions'

const REWIND_GROUP_COUNT = 4n
const CLOSED_GROUP_POLL_MS = 200
const TRACK_STATUS_POLL_MS = 500
const AUDIO_GROUP_CLOSE_WAIT_MS = 2_000

export type ReviewWindow = {
  start: bigint
  nextGroup: bigint
  frames: ReviewFrame[]
  audio: ReviewFrame[]
}

type ReviewWindowFailure = FetchFailure & { start: bigint }

export type ReviewFetchWindow = { video?: bigint; audio?: bigint }

export type ReviewHost = {
  playWindow(window: ReviewWindow, session: ReviewSession): Promise<boolean>
  windowStarted(): void
  fastForwardDrained(): boolean
  observeLiveVideoGroup(groupId: bigint): void
  resumeLiveForward(): void
  goLive(): void
  seek(captureMicros: number): void
  failed(description: string): void
}

/// One review, from a seek until the next seek or the way back to live.
/// Nothing it started may act once it has ended.
export class ReviewSession {
  readonly originMicros: number
  readonly anchorMicros: number
  playheadMicros: number
  behindSeconds = 0
  readonly fetchWindows: ReviewFetchWindow[] = []
  private ended = false
  private audioPlayedThroughMicros = Number.NEGATIVE_INFINITY
  readonly isCurrent = (): boolean => !this.ended

  constructor(
    private readonly context: TrackContext,
    private readonly timeline: SeekTimeline,
    private readonly subscriptions: TrackSubscriptions,
    private readonly host: ReviewHost,
    private readonly target: GroupMark,
    captureMicros: number
  ) {
    this.originMicros = target.captureMicros
    this.anchorMicros = Math.max(captureMicros, target.captureMicros)
    this.playheadMicros = this.anchorMicros
  }

  start(): void {
    void this.pollLiveEdge()
    void this.run()
  }

  end(): void {
    this.ended = true
  }

  /// Review playback is paced by capture timestamps, so it trails the live edge
  /// until the viewer asks to go back. The FETCH for the next window is issued
  /// while the current one plays, so a window boundary does not stall on the
  /// request.
  private async run(): Promise<void> {
    let pending = await this.fetchWindow(this.target.groupId)
    while (pending && this.isCurrent()) {
      if (!('frames' in pending)) {
        this.recover(pending)
        return
      }
      const upcoming = this.fetchWindow(pending.nextGroup)
      this.behindSeconds = this.timeline.groups.secondsBehindLive(pending.start)
      this.host.windowStarted()
      const played = await this.host.playWindow(
        { ...pending, frames: sortReviewFrames(pending.frames), audio: this.unplayedAudio(pending.audio) },
        this
      )
      pending = played ? await upcoming : undefined
    }
  }

  /// Without live delivery the timeline would stop at the review's start, so
  /// TRACK_STATUS stands in for it: the groups it reports as the largest are the
  /// ones review may fetch up to next. A relay that cannot answer gets the live
  /// subscriptions forwarding again.
  private async pollLiveEdge(): Promise<void> {
    while (this.isCurrent() && this.subscriptions.paused) {
      try {
        const [videoGroup, audioGroup] = await Promise.all([
          this.largestLiveGroup('video'),
          this.largestLiveGroup('audio')
        ])
        if (!this.isCurrent()) {
          return
        }
        if (videoGroup !== undefined) {
          this.host.observeLiveVideoGroup(videoGroup)
        }
        if (audioGroup !== undefined) {
          this.timeline.audio.recordLiveGroup(audioGroup)
        }
      } catch (error) {
        this.context.log('warn', `track status: ${getErrorMessage(error)}; live subscriptions forward during review`)
        this.host.resumeLiveForward()
        return
      }
      await new Promise((resolve) => setTimeout(resolve, TRACK_STATUS_POLL_MS))
    }
  }

  private async largestLiveGroup(kind: MediaKind): Promise<bigint | undefined> {
    const name = this.subscriptions.get(kind)?.name
    if (!name) {
      return undefined
    }
    const status = await this.context.client.trackStatus(this.context.namespace, name, '')
    return status.contentExists ? status.largestGroupId : undefined
  }

  private recover(failure: ReviewWindowFailure): void {
    const reason = `fetch from group ${failure.start}: ${failure.description}`
    if (!failure.evicted) {
      this.host.failed(failure.description)
      this.context.log('error', reason)
      return
    }
    this.timeline.groups.forgetThrough(failure.start)
    const next = this.timeline.groups.resolveSeekTarget(0)
    if (!next) {
      this.context.log('warn', `${reason}; no later group is cached, going live`)
      this.host.goLive()
      return
    }
    this.context.log('warn', `${reason}; resuming from group ${next.groupId}`)
    this.host.seek(next.captureMicros)
  }

  private async fetchWindow(start: bigint): Promise<ReviewWindow | ReviewWindowFailure | undefined> {
    const end = await this.awaitClosedWindowEnd(start)
    const subscription = this.subscriptions.get('video')
    if (end === undefined || !subscription) {
      return undefined
    }
    const audioName = this.subscriptions.get('audio')?.name
    const [frames, audio] = await Promise.all([
      fetchFrames(this.context, subscription.name, start, end, this.isCurrent),
      audioName ? this.fetchAudio(audioName, start, end) : Promise.resolve([])
    ])
    if (!frames || !audio) {
      return undefined
    }
    if (isFetchFailure(frames)) {
      return { start, ...frames }
    }
    if (frames.length === 0) {
      return { start, evicted: true, description: 'no cached objects' }
    }
    if (isFetchFailure(audio)) {
      this.context.log('error', `fetch ${audioName}: ${audio.description}`)
    }
    const audioFrames = isFetchFailure(audio) ? [] : audio
    this.context.log('info', `fetched ${frames.length} objects from group ${start}`)
    this.fetchWindows.push({ video: frames[0]?.requestId, audio: audioFrames[0]?.requestId })
    return { start, nextGroup: end + 1n, frames, audio: sortReviewFrames(audioFrames) }
  }

  private async fetchAudio(
    trackName: string,
    start: bigint,
    end: bigint
  ): Promise<ReviewFrame[] | FetchFailure | undefined> {
    const range = await this.awaitAudioRange(start, end)
    if (!this.isCurrent()) {
      return undefined
    }
    if (!range) {
      this.context.log('warn', `no audio group covers video groups ${start}-${end}`)
      return []
    }
    return fetchFrames(this.context, trackName, range.start, range.end, this.isCurrent)
  }

  /// The audio of a group ends a little after its video: the source interleaves
  /// audio behind video, so audio captured just before a keyframe arrives after
  /// that keyframe has opened the next group. The audio groups are fetched once
  /// the audio track has moved on past the window, or after a bounded wait. When
  /// the audio groups share the ids of the video groups the window's audio is
  /// the same group range; otherwise it is the range of audio groups whose
  /// capture times cover the window's.
  private async awaitAudioRange(start: bigint, end: bigint): Promise<GroupRange | undefined> {
    const { audio, groups } = this.timeline
    const fromMicros = groups.startOf(start)
    const untilMicros = groups.startAfter(end)
    const byCapture =
      fromMicros !== undefined &&
      untilMicros !== undefined &&
      !audio.sharesVideoIds((groupId) => groups.startOf(groupId))
    const movedOn = () =>
      byCapture ? audio.closedUntil(untilMicros) : audio.newestGroupId !== undefined && audio.newestGroupId > end
    const deadline = performance.now() + AUDIO_GROUP_CLOSE_WAIT_MS
    while (this.isCurrent() && !movedOn() && performance.now() < deadline) {
      await new Promise((resolve) => setTimeout(resolve, CLOSED_GROUP_POLL_MS))
    }
    return byCapture ? audio.covering(fromMicros, untilMicros) : { start, end }
  }

  /// Audio groups that cover consecutive windows are fetched for each of them,
  /// so what an earlier window has already played is left out.
  private unplayedAudio(audio: ReviewFrame[]): ReviewFrame[] {
    const unplayed = audio.filter(
      (chunk) => chunk.captureMicros === undefined || chunk.captureMicros > this.audioPlayedThroughMicros
    )
    this.audioPlayedThroughMicros = unplayed.reduce(
      (through, chunk) => Math.max(through, chunk.captureMicros ?? through),
      this.audioPlayedThroughMicros
    )
    return unplayed
  }

  /// The live edge group is still open and a FETCH that reaches into it escapes
  /// the relay cache, so a window can only end at the newest closed group. Once
  /// playback has consumed those, wait for the publisher to close another one;
  /// at more than real time that wait would recur on every group from then on,
  /// so review that has played out everything fetched goes live instead.
  private async awaitClosedWindowEnd(start: bigint): Promise<bigint | undefined> {
    while (this.isCurrent()) {
      const newestClosed = this.timeline.groups.newestClosed
      if (newestClosed && start <= newestClosed.groupId) {
        const bounded = start + REWIND_GROUP_COUNT
        return bounded < newestClosed.groupId ? bounded : newestClosed.groupId
      }
      if (newestClosed && this.host.fastForwardDrained()) {
        this.host.goLive()
        return undefined
      }
      await new Promise((resolve) => setTimeout(resolve, CLOSED_GROUP_POLL_MS))
    }
    return undefined
  }
}
