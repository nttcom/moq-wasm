import type { MoqtClientWrapper } from '@moqt/moqtClient'
import type { SubgroupObjectHandler } from '@moqt/subscriptionStateManager'
import { readLocHeader } from '../../../utils/media/loc'
import type { StreamMonitor } from '../streamMonitor'

export type Packaging = 'loc' | 'cmaf'

export type MediaKind = 'video' | 'audio'

export type LogLevel = 'info' | 'warn' | 'error'

export type DeliveryObserver = Pick<
  StreamMonitor,
  'label' | 'object' | 'fetchObject' | 'fetchFinished' | 'forget' | 'setPlayhead' | 'clearPlayhead'
>

export type TrackContext = {
  client: MoqtClientWrapper
  namespace: string[]
  authInfo: string
  observer: DeliveryObserver
  log: (level: LogLevel, message: string) => void
}

export function observedObjectHandler(
  context: TrackContext,
  trackAlias: bigint,
  track: string,
  handler: SubgroupObjectHandler
): SubgroupObjectHandler {
  context.observer.label(trackAlias, track)
  return (groupId, object) => {
    context.observer.object(
      trackAlias,
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
