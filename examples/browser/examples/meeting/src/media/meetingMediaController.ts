import { MoqtClientWrapper, RequestErrorCode } from '@moqt/moqtClient'
import { MediaPublisher } from './mediaPublisher'
import { MediaSubscriber } from './mediaSubscriber'

export class MeetingMediaController {
  readonly publisher: MediaPublisher
  readonly subscriber: MediaSubscriber

  constructor(
    client: MoqtClientWrapper,
    private readonly trackNamespace: string[]
  ) {
    this.publisher = new MediaPublisher(client, trackNamespace)
    this.subscriber = new MediaSubscriber(client)

    client.setOnIncomingSubscribeHandler(async ({ subscribe, isSuccess, code, respondOk, respondError }) => {
      const trackName = subscribe.trackName ?? ''
      const isLocalTrack = this.isLocalNamespace(subscribe.trackNamespace)
      const isCatalogTrack = this.publisher.isCatalogTrack(trackName)
      console.info('[meeting][moqt] received SUBSCRIBE', { subscribe, isSuccess, code })
      if (!isSuccess) {
        await respondError(BigInt(code), 'Subscription validation failed')
        return
      }
      if (!isLocalTrack) {
        await respondError(404n, 'Unknown namespace')
        return
      }
      if (trackName === 'chat') {
        await respondOk(0n)
        return
      }
      if (isCatalogTrack) {
        const trackAlias = await respondOk(0n)
        await this.publisher.sendCatalogToAlias(trackAlias)
        return
      }
      const role = this.publisher.resolveTrackRole(trackName)
      if (!role) {
        await respondError(404n, 'Unknown track')
        return
      }
      await respondOk(0n)
      try {
        if (role === 'video') {
          await this.publisher.applyVideoEncodingForTrack(trackName)
          this.publisher.forceVideoKeyframeForTrack(trackName)
        } else if (role === 'audio') {
          await this.publisher.applyAudioEncodingForTrack(trackName)
        }
      } catch (err) {
        console.error('Failed to kick media pipeline for new subscriber', err)
      }
    })

    client.setOnIncomingFetchHandler(async (context) => {
      const { fetch } = context
      if (this.isLocalNamespace(fetch.trackNamespace) && this.publisher.isCatalogTrack(fetch.trackName)) {
        await this.publisher.answerCatalogFetch(context)
        return
      }
      await context.respondError(RequestErrorCode.NotSupported, 'fetch is served for the catalog only')
    })

    client.setOnIncomingUnsubscribeHandler((subscribeId) => {
      console.info('[meeting][moqt] received UNSUBSCRIBE', { subscribeId: subscribeId.toString() })
      this.publisher.handleIncomingUnsubscribe(subscribeId)
    })
  }

  async dispose(): Promise<void> {
    await this.publisher.dispose()
    this.subscriber.dispose()
  }

  private isLocalNamespace(trackNamespace: string[]): boolean {
    if (trackNamespace.length !== this.trackNamespace.length) {
      return false
    }
    return trackNamespace.every((value, index) => value === this.trackNamespace[index])
  }
}
