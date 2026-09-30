import { getErrorMessage } from '../../examples/media/common'
import { type TrackContext, observedObjectHandler } from './trackContext'
import type { SubscribeOkMessage } from '../../pkg/moqt_client_wasm'

export type TextTrackHandler = (text: string, groupId: bigint) => void

export class TextTracks {
  private readonly subscribed: { requestId: bigint; trackAlias: bigint }[] = []

  constructor(private readonly context: TrackContext) {}

  async subscribe(name: string, onText: TextTrackHandler): Promise<SubscribeOkMessage> {
    const { client, namespace, authInfo } = this.context
    const { requestId, subscribeOk } = await client.subscribe(namespace, name, authInfo, { forward: true })
    this.subscribed.push({ requestId, trackAlias: subscribeOk.trackAlias })
    client.setOnSubgroupObjectHandler(
      subscribeOk.trackAlias,
      observedObjectHandler(this.context, subscribeOk.trackAlias, name, (groupId, object) =>
        deliverText(object.objectPayload, groupId, onText)
      )
    )
    this.context.log('info', `subscribed ${namespace.join('/')}/${name}`)
    return subscribeOk
  }

  async unsubscribeAll(): Promise<void> {
    const { client } = this.context
    for (const { requestId, trackAlias } of this.subscribed.splice(0)) {
      client.clearSubgroupObjectHandler(trackAlias)
      if (client.getConnectionStatus()) {
        await client.unsubscribe(requestId)
      }
    }
  }
}

/// draft-ietf-moq-transport-14 §9.8: SUBSCRIBE_OK names a Largest Location
/// only when content exists; without one nothing has been published yet, so
/// there is nothing to fetch and the first object arrives on the SUBSCRIBE.
export async function fetchLatestText(
  context: TrackContext,
  name: string,
  subscribeOk: SubscribeOkMessage,
  onText: TextTrackHandler
): Promise<void> {
  const largestGroup = subscribeOk.largestGroupId
  if (largestGroup === undefined) {
    context.log('info', `${name} has no published object yet; waiting for it on the subscription`)
    return
  }
  const endObject = (subscribeOk.largestObjectId ?? 0n) + 1n
  try {
    const { requestId } = await context.client.fetch(
      context.namespace,
      name,
      largestGroup,
      0n,
      largestGroup,
      endObject,
      {
        onObject: (message) => {
          context.observer.fetchObject(
            message.requestId,
            name,
            message.groupId,
            message.objectId,
            message.objectPayload.byteLength
          )
          deliverText(message.objectPayload, message.groupId, onText)
        }
      }
    )
    context.observer.fetchFinished(requestId)
    context.log('info', `fetched ${name}`)
  } catch (error) {
    context.log('info', `fetch ${name}: ${getErrorMessage(error)}`)
  }
}

function deliverText(payload: Uint8Array, groupId: bigint, onText: TextTrackHandler): void {
  if (payload.byteLength > 0) {
    onText(new TextDecoder().decode(payload), groupId)
  }
}
