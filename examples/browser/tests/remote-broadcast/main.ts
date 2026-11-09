import { MoqSession } from 'moq-wasm'
import { RemoteBroadcast, type SourceView } from 'moq-wasm/media'
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

const api = { start, snapshot, move, events: () => [...events] }

declare global {
  interface Window {
    remoteBroadcastE2E: typeof api
  }
}

window.remoteBroadcastE2E = api
