import type { ReviewClock } from './reviewClock'

/// The capture timestamps a player can play, in microseconds: from the oldest
/// group its media timeline lists (`startMicros`) to the end of a VOD broadcast
/// (`endMicros`). A live range runs on past `endMicros`, its live edge.
export type PlayableRange = { startMicros: number; endMicros: number; live: boolean }

/// How a `PlaybackGroup` drives one of its players.
export type GroupMember = {
  /// Undefined until the player's seek axis is known.
  range(): PlayableRange | undefined
  /// The capture timestamp on screen while live, which a skip measures from.
  livePositionMicros(): number | undefined
  /// Throws when the player is in another group already.
  join(link: GroupLink): void
  /// `positionMicros` is the group's position, undefined while the group is
  /// live. A player that was not live plays on alone from there.
  leave(positionMicros: number | undefined): void
  goLive(): void
  /// Plays from `positionMicros` on the group's clock. The group calls it only
  /// with a position in the player's range, after anchoring the clock.
  align(positionMicros: number): void
  /// Fetches nothing: `positionMicros` lies outside its range.
  hold(positionMicros: number): void
  setPaused(paused: boolean): void
}

/// How a player reaches the group it is in.
export type GroupLink = {
  readonly clock: ReviewClock
  seek(captureMicros: number): void
  skip(seconds: number): void
  goLive(): void
  setPaused(paused: boolean): void
  /// The player has left the place the group gave it: its review found its
  /// range evicted, or its publisher or video track changed.
  placeAgain(): void
}

const members = new WeakMap<object, GroupMember>()

export function registerGroupMember(player: object, member: GroupMember): void {
  members.set(player, member)
}

export function groupMemberOf(player: object): GroupMember {
  const member = members.get(player)
  if (!member) {
    throw new Error('not a LivePlayer')
  }
  return member
}
