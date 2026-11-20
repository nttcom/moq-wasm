import { expect, test } from '@playwright/test'
import { type GroupLink, type GroupMember, type PlayableRange, registerGroupMember } from './groupMember'
import type { LivePlayer } from './livePlayer'
import { PlaybackGroup } from './playbackGroup'
import { FakeMediaEnvironment } from './testing/fakeMediaEnvironment'

const SECOND = 1_000_000
const T0 = 1_759_924_100_000_000
type Call = { kind: 'live' | 'align' | 'hold' | 'leave'; positionMicros?: number }

/// Records how the group drives it; `playable` and `shownMicros` stand for the
/// player's seek axis and the live frame it shows.
class FakeMember implements GroupMember {
  readonly calls: Call[] = []
  link: GroupLink | undefined
  paused = false

  constructor(
    public playable: PlayableRange | undefined,
    public shownMicros?: number
  ) {}

  range(): PlayableRange | undefined {
    return this.playable
  }

  livePositionMicros(): number | undefined {
    return this.shownMicros
  }

  join(link: GroupLink): void {
    this.link = link
  }

  leave(positionMicros: number | undefined): void {
    this.calls.push({ kind: 'leave', positionMicros })
  }

  goLive(): void {
    this.calls.push({ kind: 'live' })
  }

  align(positionMicros: number): void {
    this.calls.push({ kind: 'align', positionMicros })
  }

  hold(positionMicros: number): void {
    this.calls.push({ kind: 'hold', positionMicros })
  }

  setPaused(paused: boolean): void {
    this.paused = paused
  }

  get kinds(): Call['kind'][] {
    return this.calls.map(({ kind }) => kind)
  }

  get last(): Call | undefined {
    return this.calls.at(-1)
  }
}

function vod(startSeconds: number, endSeconds: number): PlayableRange {
  return { startMicros: T0 + startSeconds * SECOND, endMicros: T0 + endSeconds * SECOND, live: false }
}

function live(startSeconds: number, edgeSeconds: number): PlayableRange {
  return { startMicros: T0 + startSeconds * SECOND, endMicros: T0 + edgeSeconds * SECOND, live: true }
}

/// A player standing in for a `LivePlayer`, which the unit tests cannot create.
function fakePlayer(member: FakeMember): LivePlayer {
  const player = {} as LivePlayer
  registerGroupMember(player, member)
  return player
}

function groupOf(...members: FakeMember[]): PlaybackGroup {
  const group = new PlaybackGroup()
  for (const member of members) {
    group.add(fakePlayer(member))
  }
  return group
}

let environment: FakeMediaEnvironment

test.beforeEach(() => {
  environment = FakeMediaEnvironment.install({ outputLatencySeconds: 0 })
})

test.afterEach(() => {
  environment.uninstall()
})

function advanceSeconds(seconds: number): void {
  environment.advanceTo(environment.nowMs + seconds * 1_000)
}

test('a VOD player joining starts the group at its start, and a later range holds until the position reaches it', () => {
  // Arrange
  const first = new FakeMember(vod(0, 60))
  const lateJoiner = new FakeMember(vod(1, 60))
  const group = groupOf(first, lateJoiner)

  // Act
  advanceSeconds(1.5)

  // Assert: the late joiner plays from where the group has got to, checked every 250 ms
  expect(first.calls).toEqual([{ kind: 'align', positionMicros: T0 }])
  expect(lateJoiner.kinds).toEqual(['hold', 'align'])
  expect(lateJoiner.last?.positionMicros).toBeGreaterThanOrEqual(T0 + SECOND)
  expect(lateJoiner.last?.positionMicros).toBeLessThan(T0 + 1.25 * SECOND)
  expect(group.state.mode).toBe('review')
})

test('a player is held once the position passes the end of its VOD range', () => {
  // Arrange
  const stays = new FakeMember(vod(0, 60))
  const earlyLeaver = new FakeMember(vod(0, 1))
  groupOf(stays, earlyLeaver)

  // Act
  advanceSeconds(1.5)

  // Assert
  expect(stays.kinds).toEqual(['align'])
  expect(earlyLeaver.kinds).toEqual(['align', 'hold'])
})

test('the group ends and pauses once the position passes the end of every player', () => {
  // Arrange
  const short = new FakeMember(vod(0, 1))
  const shorter = new FakeMember(vod(0, 0.5))
  const group = groupOf(short, shorter)

  // Act
  advanceSeconds(1.5)

  // Assert
  expect(group.state).toMatchObject({ mode: 'review', ended: true, paused: true })
  expect([short.paused, shorter.paused]).toEqual([true, true])
})

test('playing a group that has ended plays it from the start', () => {
  // Arrange
  const member = new FakeMember(vod(0, 1))
  const group = groupOf(member)
  advanceSeconds(1.5)

  // Act
  group.play()

  // Assert
  expect(group.state).toMatchObject({ ended: false, paused: false, positionMicros: T0 })
  expect(member.last).toEqual({ kind: 'align', positionMicros: T0 })
  expect(member.paused).toBe(false)
})

test('a seek aligns the players whose range covers it, holds the others, and starts no earlier than the earliest range', () => {
  // Arrange
  const early = new FakeMember(live(2, 60))
  const late = new FakeMember(live(5, 60))
  const group = groupOf(early, late)

  // Act
  group.seek(T0)

  // Assert
  expect(group.state.positionMicros).toBe(T0 + 2 * SECOND)
  expect(early.last).toEqual({ kind: 'align', positionMicros: T0 + 2 * SECOND })
  expect(late.last).toEqual({ kind: 'hold', positionMicros: T0 + 2 * SECOND })
})

test('a skip while live rewinds every player from the newest position one shows', () => {
  // Arrange
  const behind = new FakeMember(live(0, 60), T0 + 58 * SECOND)
  const ahead = new FakeMember(live(0, 60), T0 + 59 * SECOND)
  const group = groupOf(behind, ahead)

  // Act
  group.skip(-10)

  // Assert
  expect(group.state).toMatchObject({ mode: 'review', positionMicros: T0 + 49 * SECOND })
  expect([behind.last, ahead.last]).toEqual([
    { kind: 'align', positionMicros: T0 + 49 * SECOND },
    { kind: 'align', positionMicros: T0 + 49 * SECOND }
  ])
})

test('a seek at or past the live edge of every live player takes them live, each at its own edge', () => {
  // Arrange
  const first = new FakeMember(live(0, 60))
  const second = new FakeMember(live(0, 59))
  const group = groupOf(first, second)
  group.seek(T0 + 30 * SECOND)

  // Act
  group.seek(T0 + 60 * SECOND)

  // Assert
  expect(group.state).toMatchObject({ mode: 'live', positionMicros: undefined })
  expect([first.kinds, second.kinds]).toEqual([
    ['live', 'align', 'live'],
    ['live', 'align', 'live']
  ])
})

test('goLive does nothing while a player is VOD', () => {
  // Arrange
  const member = new FakeMember(vod(0, 60))
  const group = groupOf(member)

  // Act
  group.goLive()

  // Assert
  expect(group.state.mode).toBe('review')
  expect(member.kinds).toEqual(['align'])
})

test('a player added while the group reviews is aligned to its position and takes its pause', () => {
  // Arrange
  const member = new FakeMember(live(0, 60))
  const group = groupOf(member)
  group.seek(T0 + 30 * SECOND)
  group.pause()
  const added = new FakeMember(live(0, 60))

  // Act
  group.add(fakePlayer(added))

  // Assert
  expect(added.calls).toEqual([{ kind: 'align', positionMicros: T0 + 30 * SECOND }])
  expect(added.paused).toBe(true)
})

test('a removed player is told the position to play on from alone, and the group stops placing it', () => {
  // Arrange
  const stays = new FakeMember(live(0, 60))
  const leaves = new FakeMember(live(0, 60))
  const leavingPlayer = fakePlayer(leaves)
  const group = new PlaybackGroup()
  group.add(fakePlayer(stays))
  group.add(leavingPlayer)
  group.seek(T0 + 30 * SECOND)
  group.pause()

  // Act
  group.remove(leavingPlayer)
  group.seek(T0 + 40 * SECOND)

  // Assert
  expect(leaves.calls.slice(-2)).toEqual([
    { kind: 'align', positionMicros: T0 + 30 * SECOND },
    { kind: 'leave', positionMicros: T0 + 30 * SECOND }
  ])
  expect(stays.last).toEqual({ kind: 'align', positionMicros: T0 + 40 * SECOND })
})

test('pausing stops the position and pauses every player', () => {
  // Arrange
  const first = new FakeMember(live(0, 60))
  const second = new FakeMember(live(0, 60))
  const group = groupOf(first, second)
  group.seek(T0 + 30 * SECOND)
  advanceSeconds(1)
  const pausedAt = group.state.positionMicros ?? 0

  // Act
  group.pause()
  advanceSeconds(2)

  // Assert
  expect(pausedAt).toBeGreaterThan(T0 + 30 * SECOND)
  expect(group.state.positionMicros).toBe(pausedAt)
  expect([first.paused, second.paused]).toEqual([true, true])
})

test('a player that loses its place, as when its range is evicted, is placed again', () => {
  // Arrange
  const member = new FakeMember(live(0, 60))
  groupOf(member)
  member.link?.seek(T0 + 30 * SECOND)
  member.playable = live(40, 60)

  // Act
  member.link?.placeAgain()

  // Assert
  expect(member.last).toEqual({ kind: 'hold', positionMicros: T0 + 30 * SECOND })
})
