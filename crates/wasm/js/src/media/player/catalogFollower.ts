import type { PublishDone } from '../../core/session'
import type { TextTracks } from './textTrack'
import { CATALOG_TRACK_NAME } from '../catalog'

/// A SUBSCRIBE delivers objects published after the largest one and the bridge
/// publishes the catalog once per upstream subscription, so a viewer joining a
/// subscription the relay already holds would never see it; the group the
/// largest object is in is read through a joining FETCH as well. The relay
/// keeps the catalog of a publisher that has since been replaced, so the
/// catalog of the newest group wins.
export class CatalogFollower {
  private newestGroupId: bigint | undefined

  constructor(
    private readonly textTracks: TextTracks,
    private readonly onCatalog: (text: string) => void,
    private readonly onEnded: (done: PublishDone) => void
  ) {}

  async follow(): Promise<void> {
    const onText = (text: string, groupId: bigint) => {
      if (this.newestGroupId !== undefined && groupId < this.newestGroupId) {
        return
      }
      this.newestGroupId = groupId
      this.onCatalog(text)
    }
    await this.textTracks.subscribe(CATALOG_TRACK_NAME, onText, { currentGroup: true, onEnded: this.onEnded })
  }

  reset(): void {
    this.newestGroupId = undefined
  }
}
