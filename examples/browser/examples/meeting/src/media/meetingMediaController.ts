import type { MoqtClientWrapper } from '@moqt/moqtClient'
import { MediaPublisher } from './mediaPublisher'
import { MediaSubscriber } from './mediaSubscriber'

export class MeetingMediaController {
  readonly publisher: MediaPublisher
  readonly subscriber: MediaSubscriber

  constructor(client: MoqtClientWrapper, trackNamespace: string[]) {
    this.publisher = new MediaPublisher(trackNamespace)
    this.subscriber = new MediaSubscriber(client)
  }

  async dispose(): Promise<void> {
    await this.publisher.dispose()
    this.subscriber.dispose()
  }
}
