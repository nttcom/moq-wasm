import { type GroupLink, type GroupMember, type PlayableRange, groupMemberOf } from './groupMember'
import type { LivePlayer } from './livePlayer'
import { ReviewClock } from './reviewClock'

const MICROS_PER_SECOND = 1_000_000
/// How often the group checks which players' ranges cover its position.
const PLACE_INTERVAL_MS = 250

export type PlaybackGroupCallbacks = {
  /// Called when the mode, the pause or the end changes, and on every seek;
  /// the position moves on without a call.
  onStateChange(): void
}

export type PlaybackGroupState = {
  mode: 'live' | 'review'
  /// The capture timestamp, in microseconds, every player in range presents;
  /// undefined while live.
  positionMicros: number | undefined
  paused: boolean
  /// Set once every player is VOD and the position has passed the end of
  /// every range; ending pauses the group.
  ended: boolean
}

/// `placement` is where the group has put the player: live at its own live
/// edge, playing from the group's position, or holding because the position
/// lies outside its range; undefined until the group places it.
type Entry = { member: GroupMember; placement: 'live' | 'aligned' | 'held' | undefined }

/// Plays several `LivePlayer`s at one position, on one clock and one
/// AudioContext: a meeting rewound for everyone, or a recorded meeting played
/// back with everyone in step. The position is the LOC capture timestamp,
/// which the publishers stamp from one wall clock. While live, each player
/// plays at its own live edge; the players are aligned only while rewinding
/// and while playing VOD.
///
/// A player in the group is operated through it: its own `seek`, `skip`,
/// `goLive` and `setPaused` act on the whole group. To rewind one player
/// alone, remove it; adding it back aligns it to the group's position.
/// A player whose range does not cover the position fetches nothing and
/// reports `inRange: false` until the position reaches its range. This lets a
/// participant who joined late, left early or rejoined under another
/// namespace play only for its part.
export class PlaybackGroup {
  private readonly clock = new ReviewClock()
  private readonly members = new Map<LivePlayer, Entry>()
  private reviewing = false
  private paused = false
  private ended = false
  private timer: ReturnType<typeof setTimeout> | undefined

  constructor(private readonly callbacks?: PlaybackGroupCallbacks) {}

  get state(): PlaybackGroupState {
    return {
      mode: this.reviewing ? 'review' : 'live',
      positionMicros: this.reviewing ? this.clock.positionMicros() : undefined,
      paused: this.paused,
      ended: this.ended
    }
  }

  /// The player takes the group's pause and position. A VOD player joining a
  /// live group starts the group at the earliest start of the known ranges,
  /// the same way a VOD player starts once its media timeline arrives. Throws
  /// when the player is in another group.
  add(player: LivePlayer): void {
    if (this.members.has(player)) {
      return
    }
    const entry: Entry = { member: groupMemberOf(player), placement: undefined }
    entry.member.join(this.linkFor(entry))
    this.members.set(player, entry)
    entry.member.setPaused(this.paused)
    if (!this.reviewing && entry.member.range()?.live === false) {
      this.seek(0)
      return
    }
    this.place(entry)
    this.watch()
    this.changed()
  }

  /// The player plays on alone from the group's position.
  remove(player: LivePlayer): void {
    const entry = this.members.get(player)
    if (!entry) {
      return
    }
    this.members.delete(player)
    entry.member.leave(this.state.positionMicros)
    this.changed()
  }

  /// Resuming a group that has ended plays it from the start.
  play(): void {
    this.setPaused(false)
  }

  /// Every player holds its picture, and a seek or skip keeps the pause.
  pause(): void {
    this.setPaused(true)
  }

  /// A position at or past the live edge of every live player goes live. One
  /// before every player's range plays from the earliest start. Each player
  /// whose range covers the position plays from it; the others hold.
  seek(captureMicros: number): void {
    const ranges = this.ranges()
    if (ranges.length === 0) {
      return
    }
    const liveEdges = ranges.filter(({ live }) => live).map(({ endMicros }) => endMicros)
    if (liveEdges.length > 0 && liveEdges.every((edgeMicros) => captureMicros >= edgeMicros)) {
      this.goLive()
      return
    }
    const positionMicros = Math.max(captureMicros, Math.min(...ranges.map(({ startMicros }) => startMicros)))
    this.reviewing = true
    this.ended = false
    this.clock.anchor(positionMicros, this.paused)
    this.placeAll()
    this.watch()
    this.changed()
  }

  /// Skips from the position, or while live from the newest position a player
  /// shows.
  skip(seconds: number): void {
    const fromMicros = this.reviewing ? this.clock.positionMicros() : this.newestLivePositionMicros()
    if (fromMicros !== undefined) {
      this.seek(fromMicros + seconds * MICROS_PER_SECOND)
    }
  }

  /// Does nothing while a player is VOD, which has no live edge.
  goLive(): void {
    if ([...this.members.values()].some(({ member }) => member.range()?.live === false)) {
      return
    }
    this.reviewing = false
    this.ended = false
    this.paused = false
    this.clock.reset()
    this.placeAll()
    this.changed()
  }

  private setPaused(paused: boolean): void {
    const restart = !paused && this.ended
    this.paused = paused
    if (this.reviewing) {
      this.clock.setPaused(paused)
    }
    for (const { member } of this.members.values()) {
      member.setPaused(paused)
    }
    if (restart) {
      this.seek(0)
    } else {
      this.changed()
    }
  }

  /// An aligned player is held once its range is gone or the position has
  /// passed the end of its VOD range. It plays on while the position lies just
  /// before the start of its range, since the sound's drift can move the
  /// position back. Going live resumes a player, so it takes the group's pause
  /// again.
  private place(entry: Entry): void {
    const { member, placement } = entry
    if (!this.reviewing) {
      if (placement !== 'live') {
        entry.placement = 'live'
        member.goLive()
        member.setPaused(this.paused)
      }
      return
    }
    const positionMicros = this.clock.positionMicros()
    if (positionMicros === undefined) {
      return
    }
    const range = member.range()
    if (placement === 'aligned') {
      if (!range || (!range.live && positionMicros > range.endMicros)) {
        entry.placement = 'held'
        member.hold(positionMicros)
      }
      return
    }
    if (range && covers(range, positionMicros)) {
      entry.placement = 'aligned'
      member.align(positionMicros)
    } else if (placement !== 'held') {
      entry.placement = 'held'
      member.hold(positionMicros)
    }
  }

  private placeAll(): void {
    for (const entry of this.members.values()) {
      entry.placement = undefined
      this.place(entry)
    }
  }

  /// The position moves on with nobody calling, so it is checked against the
  /// players' ranges for as long as the group reviews.
  private watch(): void {
    if (this.timer !== undefined || !this.reviewing || this.members.size === 0) {
      return
    }
    this.timer = setTimeout(() => {
      this.timer = undefined
      if (!this.reviewing) {
        return
      }
      for (const entry of this.members.values()) {
        this.place(entry)
      }
      this.endIfPastEveryRange()
      this.watch()
    }, PLACE_INTERVAL_MS)
  }

  private endIfPastEveryRange(): void {
    const positionMicros = this.clock.positionMicros()
    if (this.ended || this.paused || positionMicros === undefined || this.members.size === 0) {
      return
    }
    const past = [...this.members.values()].every(({ member }) => {
      const range = member.range()
      return range !== undefined && !range.live && positionMicros >= range.endMicros
    })
    if (past) {
      this.ended = true
      this.setPaused(true)
    }
  }

  private ranges(): PlayableRange[] {
    return [...this.members.values()].flatMap(({ member }) => member.range() ?? [])
  }

  private newestLivePositionMicros(): number | undefined {
    const positions = [...this.members.values()].flatMap(({ member }) => member.livePositionMicros() ?? [])
    return positions.length > 0 ? Math.max(...positions) : undefined
  }

  private linkFor(entry: Entry): GroupLink {
    return {
      clock: this.clock,
      seek: (captureMicros) => this.seek(captureMicros),
      skip: (seconds) => this.skip(seconds),
      goLive: () => this.goLive(),
      setPaused: (paused) => this.setPaused(paused),
      placeAgain: () => {
        entry.placement = undefined
        this.place(entry)
      }
    }
  }

  private changed(): void {
    this.callbacks?.onStateChange()
  }
}

function covers(range: PlayableRange, positionMicros: number): boolean {
  return positionMicros >= range.startMicros && (range.live || positionMicros <= range.endMicros)
}
