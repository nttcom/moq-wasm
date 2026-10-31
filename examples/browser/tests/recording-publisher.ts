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

export function measureFetchBytes(): void {
  document.documentElement.dataset.fetchBytes = '0'
  const fetch = MoqSession.prototype.fetch
  MoqSession.prototype.fetch = async function (...args) {
    const stream = await fetch.apply(this, args)
    const iterate = stream[Symbol.asyncIterator].bind(stream)
    stream[Symbol.asyncIterator] = async function* () {
      for await (const object of { [Symbol.asyncIterator]: iterate }) {
        document.documentElement.dataset.fetchBytes = String(
          Number(document.documentElement.dataset.fetchBytes) + object.objectPayloadLength
        )
        yield object
      }
    }
    return stream
  }
}
