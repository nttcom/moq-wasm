import type { MoqSession, MoqSubscription, ReceivedObject } from '../../core/session'
import { readLocHeader } from '../internal/loc'
import type { DeliveryObserver } from './deliveryObserver'

export type Packaging = 'loc' | 'cmaf'

export type MediaKind = 'video' | 'audio'

export type LogLevel = 'info' | 'warn' | 'error'

export type ObjectHandler = (groupId: bigint, object: ReceivedObject) => void

export type TrackContext = {
  /// Set by `LivePlayer.start`.
  session: MoqSession | undefined
  namespace: string[]
  observer: DeliveryObserver
  log: (level: LogLevel, message: string) => void
}

export function sessionOf(context: TrackContext): MoqSession {
  if (!context.session) {
    throw new Error('the player has not started')
  }
  return context.session
}

let nextStreamId = 1n

/// The session API hides track aliases and request ids, so the delivery
/// observer keys each subscription and FETCH by an id of the player's own.
export function streamId(): bigint {
  return nextStreamId++
}

export function observedObjectHandler(
  context: TrackContext,
  id: bigint,
  track: string,
  handler: ObjectHandler
): ObjectHandler {
  context.observer.label(id, track)
  return (groupId, object) => {
    context.observer.object(
      id,
      groupId,
      object.objectId,
      object.objectPayloadLength,
      object.objectStatus != null,
      Date.now(),
      readLocHeader(object.locHeader).captureTimestampMicros
    )
    handler(groupId, object)
  }
}

/// Reads the subgroup streams concurrently, as they arrive; a stream that fails
/// ends only itself.
export async function readGroups(
  context: TrackContext,
  id: bigint,
  subscription: MoqSubscription,
  handler: ObjectHandler
): Promise<void> {
  const streams: Promise<void>[] = []
  try {
    for await (const group of subscription.groups()) {
      context.observer.opened(id, group.groupId)
      streams.push(
        (async () => {
          for await (const object of group) {
            handler(group.groupId, object)
          }
        })().catch((error: unknown) => context.log('info', `group ${group.groupId} ended early: ${error}`))
      )
    }
  } catch (error) {
    context.log('warn', `subscription read failed: ${error}`)
  }
  await Promise.all(streams)
}
