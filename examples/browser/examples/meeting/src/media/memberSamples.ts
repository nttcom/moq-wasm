import type { StatsSample } from '../../../../utils/clientTelemetry'
import type { RemotePlayers } from './remotePlayers'
import type { RemoteMember } from '../types/member'

export function memberSamples(players: Pick<RemotePlayers, 'stats'>, members: RemoteMember[]): StatsSample[] {
  return members.flatMap((member) =>
    Object.entries(players.stats(member.id)).flatMap(([mediaSource, values]) =>
      values ? [{ trackNamespace: member.trackNamespace.join('/'), mediaSource, values }] : []
    )
  )
}
