import { MediaPublisher } from './mediaPublisher'
import { MediaSubscriber } from './mediaSubscriber'

export class MeetingMediaController {
  readonly publisher: MediaPublisher
  readonly subscriber = new MediaSubscriber()

  constructor(trackNamespace: string[]) {
    this.publisher = new MediaPublisher(trackNamespace)
  }

  async dispose(): Promise<void> {
    await this.publisher.dispose()
    this.subscriber.dispose()
  }
}
