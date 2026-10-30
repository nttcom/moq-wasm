import { type MoqtClientWrapper, RequestErrorCode } from '../../lib/moqt/moqtClient'
import { SubgroupWriter } from '../../lib/moqt/subgroupWriter'
import { CATALOG_TRACK_NAME } from 'moq-wasm/media/catalog'
import { timeSeededGroupId } from 'moq-wasm/media/internal/clock'

type MediaPublication = {
  trackNamespace: () => string[]
  catalogJson: (trackNamespace: string[]) => string
  mediaTrackNames: string[]
  onStatus: (text: string) => void
  onMediaSubscriber?: (trackName: string) => void
}

export function getTrackAliases(client: MoqtClientWrapper, trackNamespace: string[], trackNames: string[]): bigint[] {
  return [...new Set(trackNames.flatMap((name) => client.trackSubscribers(trackNamespace, name)))]
}

export function serveSubscribes(moqtClient: MoqtClientWrapper, publication: MediaPublication): void {
  let nextCatalogGroupId = timeSeededGroupId()
  moqtClient.setOnIncomingSubscribeHandler(async ({ subscribe, respondOk, respondError }) => {
    console.log({ subscribeMessage: subscribe })
    const trackNamespace = publication.trackNamespace()
    const requestedNamespace = subscribe.trackNamespace.join('/')
    if (requestedNamespace !== trackNamespace.join('/')) {
      publication.onStatus(`Rejected subscribe: unknown namespace ${requestedNamespace}`)
      await respondError(RequestErrorCode.TrackDoesNotExist, 'unknown namespace')
      return
    }

    const { trackName } = subscribe
    if (trackName === CATALOG_TRACK_NAME) {
      const writer = new SubgroupWriter(moqtClient, await respondOk(0n))
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
    await respondError(RequestErrorCode.TrackDoesNotExist, 'unknown track')
  })
}
