import { MoqSession } from 'moq-wasm'
import { type BroadcastWatch, RemoteBroadcast, type SourceView, createBroadcast, watchBroadcasts } from 'moq-wasm/media'
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

const api = { start, snapshot, move, events: () => [...events], startWatching, watched: () => [...watched] }

declare global {
  interface Window {
    remoteBroadcastE2E: typeof api
  }
}

window.remoteBroadcastE2E = api
