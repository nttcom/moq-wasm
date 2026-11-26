import type { MoqLocation, ReceivedObject } from '../../core/session'

/// An object at or before a SUBSCRIBE's largest `location` came from the
/// joining FETCH, since the SUBSCRIBE starts after the largest object.
export function isAtOrBefore(object: ReceivedObject, location: MoqLocation | undefined): boolean {
  return (
    location !== undefined &&
    (object.groupId < location.groupId || (object.groupId === location.groupId && object.objectId <= location.objectId))
  )
}
