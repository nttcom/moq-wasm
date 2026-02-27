export {
  MEDIA_CATALOG_TRACK_NAME,
  type MediaVideoProfile,
  type MediaCatalogTrack,
  extractCatalogTracks,
  extractCatalogVideoTracks
} from '../media/catalog'

import { MEDIA_DEFAULT_VIDEO_CODEC, type MediaVideoProfile } from '../media/catalog'

export const MEDIA_VIDEO_PROFILES: MediaVideoProfile[] = [
  { trackName: 'video_1080p', label: '1080p', codec: MEDIA_DEFAULT_VIDEO_CODEC, width: 1920, height: 1080 }
]

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

// TODO: 現在は映像+音声を同じ fMP4 にムキシングして単一の MoQ トラックで送信している。
// 理想的には映像と音声を別々の MoQ トラックに分離し、カタログにも音声トラックを追加すべき。
// そうすれば個別の Subscribe/優先度制御が可能になり、カタログ仕様とも整合する。
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
    height: profile.height
  }))
  const catalog: MsfCatalog = {
    version: 1,
    generatedAt: Date.now(),
    isComplete: true,
    tracks: videoTracks
  }
  return JSON.stringify(catalog)
}
