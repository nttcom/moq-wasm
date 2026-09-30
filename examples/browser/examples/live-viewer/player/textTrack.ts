import { getErrorMessage } from '../../media/common'
import { type TrackContext, observedObjectHandler } from './trackContext'
import type { SubscribeOkMessage } from '../../../pkg/moqt_client_wasm'

export type TextTrackHandler = (text: string, groupId: bigint) => void

export async function subscribeTextTrack(
  context: TrackContext,
  name: string,
  onText: TextTrackHandler
): Promise<SubscribeOkMessage> {
  const { subscribeOk } = await context.client.subscribe(context.namespace, name, context.authInfo, { forward: true })
  context.client.setOnSubgroupObjectHandler(
    subscribeOk.trackAlias,
    observedObjectHandler(context, subscribeOk.trackAlias, name, (groupId, object) =>
      deliverText(object.objectPayload, groupId, onText)
    )
  )
  context.log('info', `subscribed ${context.namespace.join('/')}/${name}`)
  return subscribeOk
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
