import type { MediaCatalogTrack } from '../../examples/media/catalog'
import { base64ToUint8Array } from '../../utils/media/base64'
import { MseSink, type MseSources, type MseTrackSource } from '../../utils/media/mseSink'
import type { MediaKind } from './trackContext'
import type { SubgroupObjectMessageWithLoc } from '@moqt/subscriptionStateManager'

export function cmafSource(track: MediaCatalogTrack): MseTrackSource | undefined {
  if (!track.initData || !track.codec) {
    return undefined
  }
  const container = track.role === 'audio' ? 'audio/mp4' : 'video/mp4'
  return { mimeType: `${container}; codecs="${track.codec}"`, initSegment: base64ToUint8Array(track.initData) }
}

/// Live CMAF playback through a MediaSource. MSE decodes from the first random
/// access point, so after a MediaSource is (re)opened live fragments are
/// dropped until one starts a group.
export class CmafLive {
  sink: MseSink | undefined
  private awaitingKeyframe = true

  /// Returns the sink it replaces, which the caller closes once the new one is
  /// on screen.
  async open(element: HTMLVideoElement, sources: MseSources): Promise<MseSink | undefined> {
    const previous = this.sink
    this.sink = await MseSink.open(element, sources)
    this.awaitingKeyframe = true
    return previous
  }

  detach(): MseSink | undefined {
    const previous = this.sink
    this.sink = undefined
    return previous
  }

  close(): void {
    this.sink?.close()
    this.sink = undefined
    this.awaitingKeyframe = true
  }

  append(kind: MediaKind, object: SubgroupObjectMessageWithLoc): void {
    if (!this.sink) {
      return
    }
    if (kind === 'video' && this.awaitingKeyframe) {
      if (object.objectId !== 0n) {
        return
      }
      this.awaitingKeyframe = false
    }
    const payload = new Uint8Array(object.objectPayload)
    if (kind === 'video') {
      this.sink.appendVideo(payload)
    } else {
      this.sink.appendAudio(payload)
    }
  }
}
