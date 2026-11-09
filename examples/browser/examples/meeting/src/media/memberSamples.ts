import type { StatsSample } from '../../../../utils/clientTelemetry'
import type { MediaSubscriber } from './mediaSubscriber'
import type { RemoteMember } from '../types/member'

export function memberSamples(subscriber: Pick<MediaSubscriber, 'stats'>, members: RemoteMember[]): StatsSample[] {
  return members.flatMap((member) =>
    Object.entries(subscriber.stats(member.id)).flatMap(([mediaSource, values]) =>
      values ? [{ trackNamespace: member.trackNamespace.join('/'), mediaSource, values }] : []
    )
  )
}
