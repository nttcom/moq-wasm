import type { MoqtClientWrapper } from '@moqt/moqtClient'
import { SubgroupWriter } from '@moqt/subgroupWriter'
import type { MOQTClient } from '../../pkg/moqt'
import { CATALOG_TRACK_NAME } from '../../lib/msf/catalog'
import { timeSeededGroupId } from '../../utils/media/clock'
import { requireRawClient } from './common'

export type MediaPublication = {
  trackNamespace: () => string[]
  catalogJson: (trackNamespace: string[]) => string
  mediaTrackNames: string[]
  onStatus: (text: string) => void
  onMediaSubscriber?: (trackName: string) => void
}

export function getTrackAliases(client: MOQTClient, trackNamespace: string[], trackNames: string[]): bigint[] {
  return [...new Set(trackNames.flatMap((name) => [...client.getTrackSubscribers(trackNamespace, name)]))]
}

export function serveSubscribes(moqtClient: MoqtClientWrapper, publication: MediaPublication): void {
  let nextCatalogGroupId = timeSeededGroupId()
  moqtClient.setOnIncomingSubscribeHandler(async ({ subscribe, isSuccess, code, respondOk, respondError }) => {
    console.log({ subscribeMessage: subscribe })
    if (!isSuccess) {
      const reasonPhrase = `subscribe error: code=${code}`
      publication.onStatus(reasonPhrase)
      await respondError(BigInt(code), reasonPhrase)
      return
    }

    const trackNamespace = publication.trackNamespace()
    const requestedNamespace = subscribe.trackNamespace.join('/')
    if (requestedNamespace !== trackNamespace.join('/')) {
      publication.onStatus(`Rejected subscribe: unknown namespace ${requestedNamespace}`)
      await respondError(404n, 'unknown namespace')
      return
    }

    const { trackName } = subscribe
    if (trackName === CATALOG_TRACK_NAME) {
      const writer = new SubgroupWriter(requireRawClient(moqtClient), await respondOk(0n))
      await writer.openGroup(nextCatalogGroupId++)
      await writer.send(new TextEncoder().encode(publication.catalogJson(trackNamespace)))
      await writer.closeGroup()
      publication.onStatus(`Catalog served: ${requestedNamespace}/${CATALOG_TRACK_NAME}`)
      return
    }
    if (publication.mediaTrackNames.includes(trackName)) {
      await respondOk(0n)
      publication.onMediaSubscriber?.(trackName)
      return
    }
    publication.onStatus(`Rejected subscribe: unknown track ${trackName}`)
    await respondError(404n, 'unknown track')
  })
}
