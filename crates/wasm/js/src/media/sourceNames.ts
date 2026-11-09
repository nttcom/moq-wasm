import { CATALOG_TRACK_NAME } from './catalog'

const AUDIO_SUFFIX = '_audio'
/// Suffixes `broadcast` gives the tracks it derives from a source's tracks.
const DERIVED_SUFFIXES = [AUDIO_SUFFIX, '_cmaf', '_timeline']
const CHAT_TRACK_NAME = 'chat'

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
