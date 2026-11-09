/// The page the PlaybackGroup E2E drives: one `LivePlayer` per namespace, all in one group.
import { MoqSession } from 'moq-wasm'
import { type DeliveryObserver, LivePlayer, PlaybackGroup, type PlaybackGroupState } from 'moq-wasm/media'
import 'moq-wasm/media/player.css'

/// The review frame a player showed last, and when.
type Shown = { captureMicros: number; atMs: number }

export type PlayerSnapshot = {
  inRange: boolean
  /// For the message of a failed assertion.
  playbackStatus: string
  /// The capture timestamp on screen now, from the frame shown last.
  presentedMicros: number | undefined
  shownAgoMs: number | undefined
  /// The seek axis, in capture microseconds.
  startMicros: number
  endMicros: number
}

export type PlaybackGroupSnapshot = { group: PlaybackGroupState; players: PlayerSnapshot[] }

type Entry = { player: LivePlayer; shown: { last?: Shown } }

const group = new PlaybackGroup()
const entries: Entry[] = []
let session: MoqSession | undefined

function noteShown(shown: Entry['shown']): DeliveryObserver {
  return {
    label: () => {},
    opened: () => {},
    object: () => {},
    fetchObject: () => {},
    fetchFinished: () => {},
    forget: () => {},
    setPlayhead: ({ kind, captureMicros }) => {
      if (kind === 'fetch') {
        shown.last = { captureMicros, atMs: performance.now() }
      }
    },
    clearPlayhead: () => {}
  }
}

async function start(url: string, namespaces: string[]): Promise<void> {
  const opened = await MoqSession.connect(url)
  session = opened
  for (const namespace of namespaces) {
    const container = document.createElement('div')
    document.getElementById('players')?.append(container)
    const shown = {}
    const player = new LivePlayer({
      container,
      callbacks: {
        onStateChange: () => {},
        onLiveFrame: () => {},
        onLog: (level, message) => console[level](`[${namespace}] ${message}`)
      },
      deliveryObserver: noteShown(shown)
    })
    entries.push({ player, shown })
    group.add(player)
    await player.start(opened, namespace.split('/'))
  }
}

function snapshot(): PlaybackGroupSnapshot {
  const nowMs = performance.now()
  return {
    group: group.state,
    players: entries.map(({ player, shown: { last } }) => {
      const { inRange, playbackStatus, seek } = player.state
      return {
        inRange,
        playbackStatus: playbackStatus.text,
        presentedMicros: last && last.captureMicros + (nowMs - last.atMs) * 1_000,
        shownAgoMs: last && nowMs - last.atMs,
        startMicros: seek.replayableStartSeconds * 1_000_000,
        endMicros: seek.liveEdgeSeconds * 1_000_000
      }
    })
  }
}

function removeAndSeek(index: number, captureMicros: number): void {
  const { player } = entries[index]
  group.remove(player)
  player.seek(captureMicros)
}

async function stop(): Promise<void> {
  for (const { player } of entries) {
    await player.stop()
  }
  session?.close()
}

const api = {
  start,
  snapshot,
  seek: (captureMicros: number) => group.seek(captureMicros),
  removeAndSeek,
  add: (index: number) => {
    group.add(entries[index].player)
    return group.state.positionMicros
  },
  stop
}

declare global {
  interface Window {
    playbackGroupE2E: typeof api
  }
}

window.playbackGroupE2E = api
