import type { MoqtClientWrapper } from '@moqt/moqtClient'
import { parse_msf_catalog_json } from '../../pkg/moqt'
import { extractCatalogAudioTracks, extractCatalogVideoTracks, type MediaCatalogTrack } from '../../lib/msf/catalog'
import { element, parseTrackNamespace, setStatusText } from '../../utils/page'

export type TrackKind = 'video' | 'audio'

export type CatalogPick = Record<TrackKind, { tracks: MediaCatalogTrack[]; selected?: MediaCatalogTrack }>

const KINDS: TrackKind[] = ['video', 'audio']
const STATUS_ID = 'catalog-track-status'

function formatTrackLabel(kind: TrackKind, track: MediaCatalogTrack): string {
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

function placeholderOption(kind: TrackKind): HTMLOptionElement {
  const option = new Option(`Catalog ${kind} tracks are not loaded yet`, '', true, true)
  option.disabled = true
  return option
}

export function setUpCatalogPicker(
  moqtClient: MoqtClientWrapper,
  form: HTMLFormElement,
  onSelect?: (kind: TrackKind, track: MediaCatalogTrack) => void
): { pick: CatalogPick; reset: () => void } {
  const pick: CatalogPick = { video: { tracks: [] }, audio: { tracks: [] } }
  const selects: Record<TrackKind, HTMLSelectElement | null> = {
    video: document.getElementById('selected-video-track') as HTMLSelectElement | null,
    audio: document.getElementById('selected-audio-track') as HTMLSelectElement | null
  }

  const choose = (kind: TrackKind, name: string | undefined) => {
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
    const { trackAlias } = (
      await moqtClient.subscribe(trackNamespace, catalogTrackName, 'secret', {
        requestId: BigInt(form['catalog-subscribe-id'].value)
      })
    ).subscribeOk
    form['catalog-track-alias'].value = trackAlias.toString()
    setStatusText(STATUS_ID, `Catalog subscribe requested: ${catalogTrackName}`)
    moqtClient.setOnSubgroupObjectHandler(trackAlias, (_groupId, object) => {
      if (object.objectStatus !== undefined) {
        return
      }
      try {
        const catalog = parse_msf_catalog_json(new TextDecoder().decode(object.objectPayload))
        pick.video.tracks = extractCatalogVideoTracks(catalog)
        pick.audio.tracks = extractCatalogAudioTracks(catalog)
        render()
      } catch (error) {
        console.error('[CatalogPicker] failed to parse catalog', error)
        setStatusText(STATUS_ID, 'Catalog parse failed')
      }
    })
  })

  render()
  return {
    pick,
    reset: () => {
      pick.video.tracks = []
      pick.audio.tracks = []
      render()
    }
  }
}
