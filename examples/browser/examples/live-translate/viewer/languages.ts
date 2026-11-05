import type { MediaCatalogTrack } from 'moq-wasm/media'

export const ORIGINAL_LANGUAGE = 'ja'

const LANGUAGE_LABELS: Record<string, string> = {
  ja: '日本語（原音声）',
  en: 'English',
  es: 'Español',
  de: 'Deutsch',
  ko: '한국어',
  'zh-Hans': '中文（简体）'
}

export type LanguageOption = { lang: string; label: string; track: MediaCatalogTrack }

/// One option per language, the original first; of several tracks of a
/// language, the one with the highest bitrate.
export function languageOptions(audioTracks: MediaCatalogTrack[]): LanguageOption[] {
  const byLanguage = new Map<string, MediaCatalogTrack>()
  for (const track of audioTracks) {
    if (!track.lang) {
      continue
    }
    const current = byLanguage.get(track.lang)
    if (!current || (track.bitrate ?? 0) > (current.bitrate ?? 0)) {
      byLanguage.set(track.lang, track)
    }
  }
  return [...byLanguage]
    .map(([lang, track]) => ({ lang, label: LANGUAGE_LABELS[lang] ?? lang, track }))
    .sort((left, right) => Number(right.lang === ORIGINAL_LANGUAGE) - Number(left.lang === ORIGINAL_LANGUAGE))
}
