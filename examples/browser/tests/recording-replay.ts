import { MoqSession } from 'moq-wasm'

/// The first object of each of `tracks` in `namespace`, parsed as JSON, in `tracks` order. Reads them as a
/// player joins a live broadcast: SUBSCRIBE_NAMESPACE on `prefix` until it announces `namespace`, then
/// SUBSCRIBE with a joining FETCH per track.
export async function readReplay(
  relay: string,
  prefix: string[],
  namespace: string[],
  tracks: string[]
): Promise<any[]> {
  const session = await MoqSession.connect(relay)
  try {
    const watch = await session.watchNamespaces(prefix)
    let announced = false
    for await (const event of watch) {
      if (event.kind === 'announced' && event.namespace.join('/') === namespace.join('/')) {
        announced = true
        break
      }
    }
    await watch.close()
    if (!announced) {
      throw new Error(`${namespace.join('/')} was not announced under ${prefix.join('/')}`)
    }
    const firstDocument = async (track: string) => {
      const subscription = await session.subscribeCurrentGroup(namespace, track)
      try {
        for await (const object of subscription) {
          return JSON.parse(new TextDecoder().decode(object.objectPayload))
        }
        throw new Error(`${track} ended without an object`)
      } finally {
        await subscription.close()
      }
    }
    const documents = []
    for (const track of tracks) {
      documents.push(await firstDocument(track))
    }
    return documents
  } finally {
    session.close()
  }
}

/// The namespaces SUBSCRIBE_NAMESPACE on `prefix` announces within `waitMs`.
export async function announcedNamespaces(relay: string, prefix: string[], waitMs: number): Promise<string[]> {
  const session = await MoqSession.connect(relay)
  try {
    const watch = await session.watchNamespaces(prefix)
    const namespaces: string[] = []
    const collected = (async () => {
      for await (const event of watch) {
        if (event.kind === 'announced') {
          namespaces.push(event.namespace.join('/'))
        }
      }
    })()
    await new Promise((resolve) => setTimeout(resolve, waitMs))
    await watch.close()
    await collected
    return namespaces
  } finally {
    session.close()
  }
}
