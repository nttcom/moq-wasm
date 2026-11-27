import {
  type MoqSession,
  type MoqSubscription,
  type ReceivedObject,
  type StreamResetCode,
  StreamResetError
} from '../../core/session'
import { readLocHeader } from '../internal/loc'
import { isAtOrBefore } from '../internal/location'
import type { DeliveryObserver } from './deliveryObserver'
import type { MediaCatalogTrack } from '../catalog'

export type Packaging = 'loc' | 'cmaf'

export type { MediaKind } from '../internal/encoders/sourceEncoding'

export type LogLevel = 'info' | 'warn' | 'error'

/// A track playback reads: `name` is the track on the wire, the CMAF sibling
/// of the catalog `track` in CMAF mode.
export type PlayedTrack = { name: string; track: MediaCatalogTrack }

/// `cached` marks an object a joining FETCH brought: it predates the
/// subscription, so its arrival says nothing about the delivery.
export type ObjectHandler = (groupId: bigint, object: ReceivedObject, cached: boolean) => void

export type ResetHandler = (groupId: bigint, code: StreamResetCode) => void

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
  return (groupId, object, cached) => {
    context.observer.object(
      id,
      groupId,
      object.objectId,
      object.objectPayloadLength,
      object.objectStatus != null,
      Date.now(),
      readLocHeader(object.locHeader).timestampMicros
    )
    handler(groupId, object, cached)
  }
}

/// Reads the subgroup streams concurrently, as they arrive; a stream that fails
/// ends only itself.
export async function readGroups(
  context: TrackContext,
  id: bigint,
  subscription: MoqSubscription,
  handler: ObjectHandler,
  onReset: ResetHandler = () => {}
): Promise<void> {
  const streams: Promise<void>[] = []
  try {
    for await (const group of subscription.groups()) {
      context.observer.opened(id, group.groupId)
      streams.push(
        (async () => {
          for await (const object of group) {
            handler(group.groupId, object, false)
          }
        })().catch((error: unknown) => {
          context.log('info', `group ${group.groupId} ended early: ${error}`)
          if (error instanceof StreamResetError) {
            onReset(group.groupId, error.code)
          }
        })
      )
    }
  } catch (error) {
    context.log('warn', `subscription read failed: ${error}`)
  }
  await Promise.all(streams)
}

/// Reads a subscription that started with a joining FETCH, object by object:
/// the objects up to the largest one SUBSCRIBE_OK reported come from the FETCH.
export async function readJoinedObjects(
  context: TrackContext,
  id: bigint,
  subscription: MoqSubscription,
  handler: ObjectHandler,
  onReset: ResetHandler = () => {}
): Promise<void> {
  let openedGroupId: bigint | undefined
  try {
    for await (const event of subscription.events()) {
      if (event.kind === 'reset') {
        context.log('info', `group ${event.groupId} ended early: reset with ${event.code}`)
        onReset(event.groupId, event.code)
        continue
      }
      const object = event.object
      if (object.groupId !== openedGroupId) {
        openedGroupId = object.groupId
        context.observer.opened(id, object.groupId)
      }
      handler(object.groupId, object, isAtOrBefore(object, subscription.largest))
    }
  } catch (error) {
    context.log('warn', `subscription read failed: ${error}`)
  }
}
