import type { MoqtClientWrapper } from '@moqt/moqtClient'
import { parse_msf_catalog_json } from '../../pkg/moqt'
import { extractCatalogTracks, type MediaCatalogTrack } from '../../lib/msf/catalog'
import { element, parseTrackNamespace, setStatusText } from '../../utils/page'
import type { MediaKind } from '../../lib/player/trackContext'

type CatalogPick = Record<MediaKind, { tracks: MediaCatalogTrack[]; selected?: MediaCatalogTrack }>

const KINDS: MediaKind[] = ['video', 'audio']
const STATUS_ID = 'catalog-track-status'

function formatTrackLabel(kind: MediaKind, track: MediaCatalogTrack): string {
  const details =
    kind === 'video'
      ? [typeof track.width === 'number' && typeof track.height === 'number' ? `${track.width}x${track.height}` : '']
      : [
          track.codec,
          typeof track.samplerate === 'number' ? `${track.samplerate}Hz` : '',
          track.channelConfig,
          typeof track.bitrate === 'number' ? `${Math.round(track.bitrate / 1000)}kbps` : ''
        ]
  const metadata = details.filter(Boolean).join(', ')
  return metadata ? `${track.label} (${metadata})` : track.label
}

function placeholderOption(kind: MediaKind): HTMLOptionElement {
  const option = new Option(`Catalog ${kind} tracks are not loaded yet`, '', true, true)
  option.disabled = true
  return option
}

export function setUpCatalogPicker(
  moqtClient: MoqtClientWrapper,
  form: HTMLFormElement,
  onSelect?: (kind: MediaKind, track: MediaCatalogTrack) => void
): { pick: CatalogPick; reset: () => void } {
  const pick: CatalogPick = { video: { tracks: [] }, audio: { tracks: [] } }
  const selects: Record<MediaKind, HTMLSelectElement | null> = {
    video: document.getElementById('selected-video-track') as HTMLSelectElement | null,
    audio: document.getElementById('selected-audio-track') as HTMLSelectElement | null
  }

  const choose = (kind: MediaKind, name: string | undefined) => {
    const track = pick[kind].tracks.find((entry) => entry.name === name)
    pick[kind].selected = track
    if (track) {
      onSelect?.(kind, track)
    }
  }

  const render = () => {
    for (const kind of KINDS) {
      const { tracks, selected } = pick[kind]
      const name = tracks.find((track) => track.name === selected?.name)?.name ?? tracks[0]?.name
      const select = selects[kind]
      if (select) {
        select.replaceChildren(
          ...(tracks.length
            ? tracks.map((track) => new Option(formatTrackLabel(kind, track), track.name))
            : [placeholderOption(kind)])
        )
        select.value = name ?? ''
      }
      choose(kind, name)
    }
    setStatusText(
      STATUS_ID,
      pick.video.tracks.length || pick.audio.tracks.length
        ? `Catalog loaded: video=${pick.video.tracks.length}, audio=${pick.audio.tracks.length}`
        : 'Catalog not loaded yet'
    )
  }

  const reset = () => {
    pick.video.tracks = []
    pick.audio.tracks = []
    render()
  }

  for (const kind of KINDS) {
    const select = selects[kind]
    select?.addEventListener('change', () => choose(kind, select.value))
  }

  element('sendCatalogSubscribeBtn').addEventListener('click', async () => {
    const trackNamespace = parseTrackNamespace(form['subscribe-track-namespace'].value)
    const catalogTrackName = form['catalog-track-name'].value.trim()
    if (!catalogTrackName) {
      setStatusText(STATUS_ID, 'Catalog track is required')
      return
    }
    // The publisher sends its catalog when it changes and every 30 s, so a
    // SUBSCRIBE alone would wait for the next one; the joining FETCH brings
    // the current one. That catalog can render before subscribeLatest
    // resolves, so set the status before the call for the catalog to replace.
    setStatusText(STATUS_ID, `Catalog subscribe requested: ${catalogTrackName}`)
    const { subscribeOk } = await moqtClient.subscribeLatest(
      trackNamespace,
      catalogTrackName,
      'secret',
      (payload) => {
        try {
          const catalog = parse_msf_catalog_json(new TextDecoder().decode(payload))
          pick.video.tracks = extractCatalogTracks(catalog, 'video')
          pick.audio.tracks = extractCatalogTracks(catalog, 'audio')
          render()
        } catch (error) {
          console.error('[CatalogPicker] failed to parse catalog', error)
          setStatusText(STATUS_ID, 'Catalog parse failed')
        }
      },
      {
        onPublishDone: () => {
          reset()
          setStatusText(STATUS_ID, 'Publisher ended the catalog track')
        }
      }
    )
    form['catalog-track-alias'].value = subscribeOk.trackAlias.toString()
  })

  render()
  return { pick, reset }
}
