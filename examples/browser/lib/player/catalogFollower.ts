import { MEDIA_CATALOG_TRACK_NAME } from '../../examples/media/catalog'
import { type TextTracks, fetchLatestText } from './textTrack'
import type { TrackContext } from './trackContext'

/// A SUBSCRIBE delivers objects published after the largest one and the bridge
/// publishes the catalog once per upstream subscription, so a viewer joining a
/// subscription the relay already holds would never see it; the group
/// SUBSCRIBE_OK names as the largest is fetched as well. The FETCH and the
/// SUBSCRIBE race, and the relay keeps the catalog of a publisher that has
/// since been replaced, so the catalog of the newest group wins whatever order
/// they arrive in.
export class CatalogFollower {
  private newestGroupId: bigint | undefined

  constructor(
    private readonly context: TrackContext,
    private readonly textTracks: TextTracks,
    private readonly onCatalog: (text: string) => void
  ) {}

  async follow(): Promise<void> {
    const onText = (text: string, groupId: bigint) => {
      if (this.newestGroupId !== undefined && groupId < this.newestGroupId) {
        return
      }
      this.newestGroupId = groupId
      this.onCatalog(text)
    }
    const subscribeOk = await this.textTracks.subscribe(MEDIA_CATALOG_TRACK_NAME, onText)
    await fetchLatestText(this.context, MEDIA_CATALOG_TRACK_NAME, subscribeOk, onText)
  }

  reset(): void {
    this.newestGroupId = undefined
  }
}
