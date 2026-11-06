import { MoqSession } from 'moq-wasm'

type Replay = { namespace: string[]; catalog: any; timeline: any }

/// Joins the first camera broadcast announced under `prefix` as a player joins a live one:
/// SUBSCRIBE_NAMESPACE, then the first object of the catalog and of `timeline` through SUBSCRIBE
/// with a joining FETCH.
export async function readReplay(relay: string, prefix: string[], timeline: string): Promise<Replay> {
  const session = await MoqSession.connect(relay)
  try {
    const watch = await session.watchNamespaces(prefix)
    let namespace: string[] | undefined
    for await (const event of watch) {
      if (event.kind === 'announced' && event.namespace.at(-2) === 'camera') {
        namespace = event.namespace
        break
      }
    }
    await watch.close()
    if (!namespace) {
      throw new Error(`no camera broadcast under ${prefix.join('/')}`)
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
    return { namespace, catalog: await firstDocument('catalog'), timeline: await firstDocument(timeline) }
  } finally {
    session.close()
  }
}
