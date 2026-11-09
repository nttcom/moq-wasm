import { CATALOG_TRACK_NAME, type LiveSource, type MediaCatalogTrack } from './catalog'

const AUDIO_SUFFIX = '_audio'
/// Suffixes `broadcast` gives the tracks it derives from a source's tracks.
const DERIVED_SUFFIXES = [AUDIO_SUFFIX, '_cmaf', '_timeline']
const CHAT_TRACK_NAME = 'chat'
/// The video and audio tracks a broadcast publishes without a source name,
/// such as `crates/broadcast`'s, read as the source `video`.
const DEFAULT_VIDEO_TRACK = 'video'
const DEFAULT_AUDIO_TRACK = 'audio'

export function audioTrackName(source: string): string {
  return `${source}${AUDIO_SUFFIX}`
}

export function sourceTrackNames(name: string, renditions: string[]): string[] {
  return [name, audioTrackName(name), ...renditions.map((rendition) => `${name}_${rendition}`)]
}

/// Why `name` cannot name a source publishing `trackNames` (see
/// `sourceTrackNames`), when another source holds the track names `taken`.
export function rejectedName(name: string, trackNames: string[], taken: Set<string>): string | undefined {
  if (name === '') {
    return 'it is empty'
  }
  if (name === CATALOG_TRACK_NAME || name === CHAT_TRACK_NAME) {
    return 'the broadcast lists a track of that name'
  }
  if (DERIVED_SUFFIXES.some((suffix) => name.endsWith(suffix))) {
    return `it ends in ${DERIVED_SUFFIXES.join(', ')}`
  }
  if (trackNames.some((track) => taken.has(track))) {
    return 'another source publishes a track of that name'
  }
  return undefined
}

/// A source the catalog lists: `video` and `audio` are the tracks it is named
/// after, absent while the catalog lists no track of that kind.
export type CatalogSource = { name: string; label: string; video?: string; audio?: string }

/// The sources of a catalog's LOC tracks (CMAF tracks are left out):
/// - Tracks of one kind sharing an altGroup (draft-ietf-moq-msf-01 §5.2.12),
///   such as renditions or translated audio, are one source.
/// - A video track names its source, or for an altGroup the shortest name in it.
///   A rendition `<name>_<rendition>` listed before its video, as when a muted
///   camera comes back, joins the source `<name>` its audio names.
/// - An audio track `<name>_audio` belongs to the source `<name>`, `audio` to
///   `video`, and any other audio track to a source of its own name; an audio
///   altGroup joins the source one of its tracks belongs to by that rule, or
///   else its shortest name's.
/// A source lasts while any of its tracks is listed, such as a camera's audio
/// while its video is muted.
export function catalogSources(video: MediaCatalogTrack[], audio: MediaCatalogTrack[]): CatalogSource[] {
  const sources = new Map<string, CatalogSource>()
  const videoGroups = altGroups(video)
  const videoNames = new Set(videoGroups.map((group) => shortestName(group).name))
  const namedByAudioOnly = audio.map((track) => audioSourceName(track.name)).filter((name) => !videoNames.has(name))
  for (const group of videoGroups) {
    const base = shortestName(group)
    const name = namedByAudioOnly.find((other) => base.name.startsWith(`${other}_`)) ?? base.name
    if (!sources.has(name)) {
      sources.set(name, { name, label: base.label, video: base.name })
    }
  }
  for (const group of altGroups(audio)) {
    const owner = group.find((track) => sources.has(audioSourceName(track.name))) ?? shortestName(group)
    const name = audioSourceName(owner.name)
    const source = sources.get(name) ?? { name, label: owner.label }
    source.audio ??= owner.name
    sources.set(name, source)
  }
  return [...sources.values()]
}

/// The tracks a `LivePlayer` plays for the source `source`, named as
/// `addSource` names them so the player finds a track the catalog lists later.
export function liveSource(source: CatalogSource): LiveSource {
  return {
    video: source.video ?? source.name,
    audio: source.audio ?? (source.name === DEFAULT_VIDEO_TRACK ? DEFAULT_AUDIO_TRACK : audioTrackName(source.name))
  }
}

function audioSourceName(track: string): string {
  if (track === DEFAULT_AUDIO_TRACK) {
    return DEFAULT_VIDEO_TRACK
  }
  return track.endsWith(AUDIO_SUFFIX) ? track.slice(0, -AUDIO_SUFFIX.length) : track
}

/// The LOC tracks in groups of one altGroup each; a track without one is a
/// group of its own. In catalog order.
function altGroups(tracks: MediaCatalogTrack[]): MediaCatalogTrack[][] {
  const groups = new Map<number | MediaCatalogTrack, MediaCatalogTrack[]>()
  for (const track of tracks) {
    if (track.packaging === 'cmaf') {
      continue
    }
    const key = track.altGroup ?? track
    groups.set(key, [...(groups.get(key) ?? []), track])
  }
  return [...groups.values()]
}

function shortestName(tracks: MediaCatalogTrack[]): MediaCatalogTrack {
  return tracks.reduce((shortest, track) => (track.name.length < shortest.name.length ? track : shortest))
}
