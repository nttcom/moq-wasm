export {
  MEDIA_CATALOG_TRACK_NAME,
  MEDIA_VIDEO_PROFILES,
  type MediaVideoProfile,
  type MediaCatalogTrack,
  extractCatalogTracks,
  extractCatalogVideoTracks,
} from '../media/catalog'

import { MEDIA_VIDEO_PROFILES } from '../media/catalog'

type MsfTrack = {
  namespace?: string
  name: string
  packaging: string
  role?: string
  isLive: boolean
  label?: string
  codec?: string
  width?: number
  height?: number
}

type MsfCatalog = {
  version?: number
  generatedAt?: number
  isComplete?: boolean
  tracks?: MsfTrack[]
}

export function buildCmafCatalogJson(trackNamespace: string[]): string {
  const namespace = trackNamespace.length > 0 ? trackNamespace.join('/') : undefined
  const videoTracks: MsfTrack[] = MEDIA_VIDEO_PROFILES.map((profile) => ({
    namespace,
    name: profile.trackName,
    packaging: 'cmaf',
    role: 'video',
    isLive: true,
    label: profile.label,
    codec: profile.codec,
    width: profile.width,
    height: profile.height,
  }))
  const catalog: MsfCatalog = {
    version: 1,
    generatedAt: Date.now(),
    isComplete: true,
    tracks: videoTracks,
  }
  return JSON.stringify(catalog)
}
