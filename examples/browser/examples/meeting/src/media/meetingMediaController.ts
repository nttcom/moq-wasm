import { MediaPublisher } from './mediaPublisher'
import { RemotePlayers } from './remotePlayers'

export class MeetingMediaController {
  readonly publisher: MediaPublisher
  readonly players = new RemotePlayers()

  constructor(trackNamespace: string[]) {
    this.publisher = new MediaPublisher(trackNamespace)
  }

  async dispose(): Promise<void> {
    await this.publisher.dispose()
    this.players.dispose()
  }
}
