import { MoqSession } from 'moq-wasm'
import {
  type BroadcastWatch,
  LivePlayer,
  RemoteBroadcast,
  type SourceView,
  createBroadcast,
  watchBroadcasts
} from 'moq-wasm/media'
import 'moq-wasm/media/player.css'
import { connectOptions } from '../../utils/access'

export type ViewSnapshot = {
  name: string
  video: string
  audio: string
  hasVideoSize: boolean
  videoObjects: number
  parent: string | undefined
}

const events: string[] = []
let remote: RemoteBroadcast | undefined

async function start(url: string, namespace: string[]): Promise<void> {
  const session = await MoqSession.connect(url, connectOptions())
  remote = new RemoteBroadcast(session, namespace)
  remote.listen({
    onSourceAdd: (view) => {
      events.push(`add ${view.name}`)
      document.getElementById('first')?.append(view.element)
    },
    onSourceRemove: (view) => events.push(`remove ${view.name} attached=${view.element.isConnected}`)
  })
}

function snapshot(): ViewSnapshot[] {
  return [...(remote?.sources.values() ?? [])].map((view: SourceView) => ({
    name: view.name,
    video: view.state.video,
    audio: view.state.audio,
    hasVideoSize: view.state.videoSize !== undefined,
    videoObjects: view.stats().videoObjects,
    parent: view.element.parentElement?.id
  }))
}

/// Moves the view in one task, as a page that enlarges a tile does.
function move(name: string, slot: string): void {
  const view = remote?.sources.get(name)
  if (view) {
    document.getElementById(slot)?.append(view.element)
  }
}

const watched: string[] = []
let watch: BroadcastWatch | undefined

async function startWatching(url: string, prefix: string[], own: string[], bot: string[]): Promise<void> {
  const session = await MoqSession.connect(url, connectOptions())
  const botSession = await MoqSession.connect(url, connectOptions())
  await createBroadcast(session, own)
  await createBroadcast(botSession, bot)
  watch = await watchBroadcasts(session, prefix, {
    filter: (namespace) => namespace.join('/') !== bot.join('/'),
    onAdd: (broadcast) => watched.push(`add ${broadcast.namespace.join('/')}`),
    onRemove: (broadcast) => watched.push(`remove ${broadcast.namespace.join('/')}`)
  })
}

/// How many times `reportError` reported each throwing callback, by callback name.
const reported = new Map<string, number>()
/// What the recording listener received, and how often the throwing `LivePlayer`
/// callbacks ran before they threw.
const heard = { events: [] as string[], audioLevels: 0, liveFrames: 0, playerAudioLevels: 0 }
let player: LivePlayer | undefined

class PageBug extends Error {}

addEventListener('error', (event) => {
  if (event.error instanceof PageBug) {
    reported.set(event.error.message, (reported.get(event.error.message) ?? 0) + 1)
    event.preventDefault()
  }
})

function bug(callback: string): never {
  throw new PageBug(callback)
}

/// Plays the broadcast as `start` does, through a listener whose every
/// callback throws ahead of one that records, and through a `LivePlayer`
/// whose every callback throws.
async function startThrowing(url: string, namespace: string[]): Promise<void> {
  const session = await MoqSession.connect(url, connectOptions())
  remote = new RemoteBroadcast(session, namespace, { onLog: () => bug('RemoteBroadcastOptions.onLog') })
  remote.listen({
    onSourceAdd: (view) => {
      document.getElementById('first')?.append(view.element)
      view.listen({ onChange: () => bug('onChange'), onPlayerChange: () => bug('onPlayerChange') })
      bug('onSourceAdd')
    },
    onSourceRemove: () => bug('onSourceRemove'),
    onAudioLevel: () => bug('onAudioLevel')
  })
  remote.listen({
    onSourceAdd: (view) => heard.events.push(`add ${view.name}`),
    onSourceRemove: (view) => heard.events.push(`remove ${view.name}`),
    onAudioLevel: () => {
      heard.audioLevels += 1
    }
  })
  // Its own session: `remote` already subscribes to this broadcast's catalog on
  // `session`, and a session subscribes to a track once.
  const playerSession = await MoqSession.connect(url, connectOptions())
  player = new LivePlayer({
    container: document.getElementById('player') as HTMLElement,
    callbacks: {
      onStateChange: () => bug('LivePlayer.onStateChange'),
      onLiveFrame: () => {
        heard.liveFrames += 1
        bug('LivePlayer.onLiveFrame')
      },
      onLog: () => bug('LivePlayer.onLog'),
      onAudioLevel: () => {
        heard.playerAudioLevels += 1
        bug('LivePlayer.onAudioLevel')
      }
    }
  })
  await player.start(playerSession, namespace)
}

function throwing() {
  return {
    reported: Object.fromEntries(reported),
    heard: { ...heard, events: [...heard.events] },
    playerVideoObjects: player?.stats().videoObjects ?? 0
  }
}

const api = {
  start,
  snapshot,
  move,
  events: () => [...events],
  startWatching,
  watched: () => [...watched],
  startThrowing,
  throwing
}

declare global {
  interface Window {
    remoteBroadcastE2E: typeof api
  }
}

window.remoteBroadcastE2E = api
