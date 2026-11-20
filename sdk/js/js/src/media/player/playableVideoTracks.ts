import type { MediaCatalogTrack } from '../catalog'
import { videoDecoderConfig } from '../internal/decoders/videoDecoderConfig'

/// The video tracks a viewer picks from: those `supported` says the browser
/// decodes. Of the renditions in one `altGroup` with the same size, only the
/// lowest-bitrate one stays, so a size sent in several codecs plays in the most
/// efficient one. A track without a codec, size or bitrate always stays. If no
/// track is decodable, all stay so the decoder reports the error.
export async function playableVideoTracks(
  tracks: MediaCatalogTrack[],
  supported: (codec: string) => Promise<boolean> = decoderSupports
): Promise<MediaCatalogTrack[]> {
  const decodable = await Promise.all(tracks.map((track) => !track.codec || supported(track.codec)))
  const playable = tracks.filter((_, index) => decodable[index])
  return playable.length === 0 ? tracks : withoutCostlierSameSize(playable)
}

function withoutCostlierSameSize(tracks: MediaCatalogTrack[]): MediaCatalogTrack[] {
  const cheapest = new Map<string, MediaCatalogTrack>()
  for (const track of tracks) {
    const key = sizeKey(track)
    const kept = key && cheapest.get(key)
    if (key && (!kept || track.bitrate! < kept.bitrate!)) {
      cheapest.set(key, track)
    }
  }
  return tracks.filter((track) => {
    const key = sizeKey(track)
    return !key || cheapest.get(key) === track
  })
}

function sizeKey({ altGroup, width, height, bitrate }: MediaCatalogTrack): string | undefined {
  return altGroup === undefined || width === undefined || height === undefined || bitrate === undefined
    ? undefined
    : `${altGroup}:${width}x${height}`
}

const decoderSupport = new Map<string, Promise<boolean>>()

function decoderSupports(codec: string): Promise<boolean> {
  let support = decoderSupport.get(codec)
  if (!support) {
    support = VideoDecoder.isConfigSupported(videoDecoderConfig(codec)).then(
      ({ supported }) => supported === true,
      () => false
    )
    decoderSupport.set(codec, support)
  }
  return support
}
