import {
  type Announcement,
  MoqSession,
  type MoqNamespaceWatch,
  type MoqSubscription,
  type ReceivedObject,
  type TrackWriter
} from 'moq-wasm'
import { setUpRelayUrlControls } from '../../utils/relayPresets'
import { element, getErrorMessage, parseTrackNamespace, buildDefaultTrackNamespace } from '../../utils/page'

let session: MoqSession | undefined
let announcement: Announcement | undefined
let watch: MoqNamespaceWatch | undefined
let writer: TrackWriter | undefined
let subscription: MoqSubscription | undefined

function value(id: string): string {
  return element<HTMLInputElement>(id).value.trim()
}

function checked(id: string): boolean {
  return element<HTMLInputElement>(id).checked
}

function namespace(id: string): string[] {
  return parseTrackNamespace(value(id))
}

function setSendStatus(message: string): void {
  element('send-status').textContent = message
}

function showReceived(text: string): void {
  const line = document.createElement('p')
  line.textContent = text
  element('received-text').prepend(line)
}

function describeObject({ groupId, objectId, objectPayload }: ReceivedObject): string {
  return `${groupId}/${objectId}: ${new TextDecoder().decode(objectPayload.subarray(0, 80))}`
}

function connected(): MoqSession {
  if (!session) {
    throw new Error('not connected')
  }
  return session
}

/// Shows `Sent <label>` once the relay accepts the request, or why it failed.
async function send(label: string, request: () => Promise<unknown>): Promise<void> {
  try {
    await request()
    setSendStatus(`Sent ${label}`)
  } catch (error) {
    setSendStatus(`Failed ${label}: ${getErrorMessage(error)}`)
  }
}

function onClick(id: string, handler: () => unknown): void {
  element(id).addEventListener('click', handler)
}

function readSubscription(received: MoqSubscription): void {
  void received.done.then((done) => done && showReceived(`[subscribe] PUBLISH_DONE ${done.code}: ${done.reason}`))
  void (async () => {
    for await (const object of received) {
      showReceived(`[subscribe] ${describeObject(object)}`)
    }
  })().catch((error: unknown) => console.warn('[message] subscription read ended', error))
}

/// Fills the FETCH fields with the whole group just written.
function fetchGroup(groupId: bigint): void {
  for (const id of ['fetch-start-group', 'fetch-end-group']) {
    element<HTMLInputElement>(id).value = groupId.toString()
  }
  for (const id of ['fetch-start-object', 'fetch-end-object']) {
    element<HTMLInputElement>(id).value = '0'
  }
}

function showWritten(groupId: bigint | undefined): void {
  if (groupId === undefined) {
    showReceived('[write] cached; no subscriber took it')
    return
  }
  showReceived(`[write] group ${groupId}`)
  fetchGroup(groupId)
}

function reset(): void {
  session = undefined
  announcement = undefined
  watch = undefined
  writer = undefined
  subscription = undefined
}

onClick('connectBtn', async () => {
  try {
    const opened = await MoqSession.connect(value('url'))
    session = opened
    void opened.closed.then(({ code, reason }) => {
      console.info('connection closed', { code, reason })
      if (session === opened) {
        reset()
      }
    })
    setSendStatus('Connected')
  } catch (error) {
    setSendStatus(`Failed to connect: ${getErrorMessage(error)}`)
  }
})

onClick('closeBtn', () => {
  session?.close()
  reset()
  setSendStatus('Closed')
})

onClick('announceBtn', () =>
  send('PUBLISH_NAMESPACE', async () => {
    announcement = await connected().announce(namespace('announce-namespace'))
  })
)

onClick('withdrawBtn', () =>
  send('PUBLISH_NAMESPACE_DONE', async () => {
    await announcement?.close()
    announcement = undefined
  })
)

onClick('watchBtn', () =>
  send('SUBSCRIBE_NAMESPACE', async () => {
    const opened = await connected().watchNamespaces(namespace('watch-prefix'))
    watch = opened
    void (async () => {
      for await (const { kind, namespace } of opened) {
        showReceived(`[namespace] ${kind} ${namespace.join('/')}`)
      }
    })()
  })
)

onClick('unwatchBtn', () =>
  send('UNSUBSCRIBE_NAMESPACE', async () => {
    await watch?.close()
    watch = undefined
  })
)

onClick('publishTrackBtn', async () => {
  try {
    const trackNamespace = namespace('publish-namespace')
    const track = value('publish-track')
    writer = await connected().publishTrack(trackNamespace, track, {
      retentionMs: Number(value('publish-retention')),
      publish: checked('publish-publish')
    })
    setSendStatus(`Published ${trackNamespace.join('/')}/${track}`)
  } catch (error) {
    setSendStatus(`Failed publishTrack: ${getErrorMessage(error)}`)
  }
})

onClick('writeBtn', () =>
  send('OBJECT', async () => {
    if (!writer) {
      throw new Error('publish a track first')
    }
    showWritten(
      await writer.write(new TextEncoder().encode(value('write-payload')), checked('write-new-group'), undefined)
    )
  })
)

onClick('writeGroupBtn', () =>
  send('OBJECT', async () => {
    if (!writer) {
      throw new Error('publish a track first')
    }
    showWritten(await writer.writeGroup(new TextEncoder().encode(value('write-payload'))))
  })
)

onClick('subscribeBtn', () =>
  send('SUBSCRIBE', async () => {
    const received = await connected().subscribe(namespace('subscribe-namespace'), value('subscribe-track'), {
      filter: value('subscribe-filter') as 'largestObject' | 'nextGroupStart',
      forward: checked('subscribe-forward'),
      priority: Number(value('subscribe-priority'))
    })
    subscription = received
    readSubscription(received)
  })
)

onClick('setForwardBtn', () =>
  send('SUBSCRIBE_UPDATE', async () => {
    if (!subscription) {
      throw new Error('subscribe first')
    }
    await subscription.setForward(checked('subscribe-forward'))
  })
)

onClick('unsubscribeBtn', () =>
  send('UNSUBSCRIBE', async () => {
    await subscription?.close()
    subscription = undefined
  })
)

onClick('fetchBtn', () =>
  send('FETCH', async () => {
    const fetched = await connected().fetch(
      namespace('fetch-namespace'),
      value('fetch-track'),
      { groupId: BigInt(value('fetch-start-group')), objectId: BigInt(value('fetch-start-object')) },
      { groupId: BigInt(value('fetch-end-group')), objectId: BigInt(value('fetch-end-object')) }
    )
    let count = 0
    for await (const object of fetched) {
      count += 1
      showReceived(`[fetch] ${describeObject(object)}`)
    }
    console.info(`FETCH stream finished after ${count} objects`)
  })
)

onClick('trackStatusBtn', () =>
  send('TRACK_STATUS', async () => {
    const largest = await connected().trackStatus(namespace('track-status-namespace'), value('track-status-track'))
    showReceived(`[track status] largest ${largest ? `${largest.groupId}/${largest.objectId}` : 'none'}`)
  })
)

setUpRelayUrlControls(element('url'), element('urlPresets'))
const defaultNamespace = buildDefaultTrackNamespace()
for (const id of [
  'announce-namespace',
  'watch-prefix',
  'publish-namespace',
  'subscribe-namespace',
  'fetch-namespace',
  'track-status-namespace'
]) {
  element<HTMLInputElement>(id).value = defaultNamespace
}
