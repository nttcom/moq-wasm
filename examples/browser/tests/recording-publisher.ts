import { MoqSession } from 'moq-wasm'

let session: MoqSession | undefined

// Publishes a fixed transcript without an ASR dependency.
export async function publishTranscript(relay: string, namespace: string[]): Promise<void> {
  session = await MoqSession.connect(relay)
  const writer = await session.publishTrack(namespace, 'transcript')
  const status = await session.publishTrack(namespace, 'status')
  await status.writeGroup(new TextEncoder().encode(JSON.stringify({ asr: 'test', state: 'ready' })))
  const payload = JSON.stringify([{ t: Date.now(), data: { speaker: 'alice', text: 'archived transcript' } }])
  await writer.writeGroup(new TextEncoder().encode(payload))
  await session.announce(namespace)
}

window.addEventListener('pagehide', () => session?.close())
