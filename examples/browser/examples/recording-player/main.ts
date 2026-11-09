import { MoqSession } from 'moq-wasm'
import {
  CATALOG_TRACK_NAME,
  type DeliveryObserver,
  LivePlayer,
  PlaybackGroup,
  extractCatalogTracks,
  parseCatalog
} from 'moq-wasm/media'
import { type Recording, openRecording } from 'moq-wasm/recording'
import 'moq-wasm/media/player.css'
import { bearerToken } from '../../utils/access'
import { startPageTelemetry } from '../../utils/pageTelemetry'
import { offeredVersions } from '../../utils/moqtVersion'
import { setUpRelayUrlControls } from '../../utils/relayPresets'
import { urlFromQuery } from '../../utils/serviceUrl'
import { EVENT_KINDS, type TimelineEvent, captionAt, eventRecords, mergeEvents } from './timeline'

/// One replay namespace, `<prefix>/<participant>/<content>/<session>`: a
/// participant's camera or screen share for one of the times they joined.
type Source = {
  player: LivePlayer
  container: HTMLElement
  /// The capture timestamp of the frame the player showed last.
  shownMicros: number | undefined
}

/// One participant's camera or screen share; it shows the one of its sources
/// whose range covers the position.
type Tile = {
  participant: string
  content: string
  element: HTMLElement
  caption: HTMLElement
  state: HTMLOutputElement
  sources: Source[]
}

type OpenedRecording = {
  session: MoqSession
  recording: Recording
  group: PlaybackGroup
  tiles: Map<string, Tile>
  /// Keyed by namespace: a renewed answer announces the namespaces again.
  sources: Map<string, Source>
  events: TimelineEvent[]
  /// Set by the first play or seek; until then the position waits at the
  /// start, so that every source has its range by the time it plays.
  started: boolean
}

const RENDER_INTERVAL_MS = 250
/// The recorder stops every track of a recording at once, so the sources that
/// lasted until then end within this of each other.
const RECORDING_END_SLACK_US = 1_000_000

const element = <T extends HTMLElement>(id: string) => document.getElementById(id) as T
const status = element<HTMLOutputElement>('status')
const play = element<HTMLButtonElement>('play')
const seek = element<HTMLInputElement>('seek')
const position = element<HTMLOutputElement>('position')
const recorder = element<HTMLInputElement>('recorder')
const relay = element<HTMLInputElement>('relay')
const recording = element<HTMLInputElement>('recording')
const token = element<HTMLInputElement>('token')
const load = element<HTMLButtonElement>('load')
const openDialog = element('open-dialog')
const openError = element('open-error')
const sidebar = element('sidebar')
const params = new URLSearchParams(location.search)
recorder.value = urlFromQuery(location.search, ['recorder']) ?? recorder.value
recording.value = params.get('id') ?? ''
token.value ||= bearerToken() ?? ''
setUpRelayUrlControls(relay, element('relayPresets'))
startPageTelemetry('recording-player', () => relay.value)
let opened: OpenedRecording | undefined
let errorText: string | undefined
/// Set while the seek bar is dragged, so that render does not reset it to the playing position.
let seeking = false
let renderQueued = false
setInterval(requestRender, RENDER_INTERVAL_MS)

function clock(seconds: number): string {
  const whole = Math.floor(seconds)
  const minutes = String(Math.floor(whole / 60) % 60).padStart(2, '0')
  const rest = `${minutes}:${String(whole % 60).padStart(2, '0')}`
  return whole >= 3600 ? `${Math.floor(whole / 3600)}:${rest}` : rest
}

function showPosition(): void {
  position.value = `${clock(Number(seek.value))} / ${clock(Number(seek.max))}`
}

function showPlaying(value: boolean): void {
  play.ariaLabel = play.title = value ? 'Pause' : 'Play'
  element('play-icon').setAttribute(
    'points',
    value ? '6 4 10 4 10 20 6 20 6 4 14 4 18 4 18 20 14 20 14 4' : '6 3 20 12 6 21 6 3'
  )
}

function avatar(name: string, size: string): HTMLElement {
  const chip = document.createElement('span')
  chip.className = `flex shrink-0 items-center justify-center rounded-full bg-blue-600 font-semibold text-white ${size}`
  chip.textContent = name.slice(0, 1).toUpperCase()
  chip.title = name
  return chip
}

function showNotice(text: string, kind: 'progress' | 'error'): void {
  const notice = element('stage-notice')
  notice.textContent = text
  notice.classList.remove('hidden')
  notice.classList.toggle('bg-slate-800/90', kind === 'progress')
  notice.classList.toggle('bg-red-600/90', kind === 'error')
}

function hideNotice(): void {
  element('stage-notice').classList.add('hidden')
}

function failure(error: unknown): void {
  errorText = error instanceof Error ? error.message : String(error)
  status.value = errorText
  showNotice(errorText, 'error')
}

/// Failures of a recording that has since been closed are not shown.
function failureOf(current: OpenedRecording): (error: unknown) => void {
  return (error) => {
    if (opened === current) {
      failure(error)
    }
  }
}

/// The newest object of a catalog, through SUBSCRIBE with a joining FETCH
/// (draft-ietf-moq-msf-01 §5).
async function currentCatalog(session: MoqSession, namespace: string[]): Promise<unknown> {
  const subscription = await session.subscribeCurrentGroup(namespace, CATALOG_TRACK_NAME)
  try {
    for await (const object of subscription) {
      return parseCatalog(new TextDecoder().decode(object.objectPayload))
    }
    throw new Error(`${namespace.join('/')} has no catalog`)
  } finally {
    await subscription.close()
  }
}

/// Every object of a track that is not live. The relay answers TRACK_STATUS
/// only for a track it is subscribed to, so the SUBSCRIBE_OK of a SUBSCRIBE
/// tells where the track ends.
async function fetchTrack(session: MoqSession, namespace: string[], name: string): Promise<Uint8Array[]> {
  const subscription = await session.subscribe(namespace, name)
  const largest = subscription.largest
  await subscription.close()
  if (!largest) {
    return []
  }
  const objects = await session.fetch(
    namespace,
    name,
    { groupId: 0n, objectId: 0n },
    { groupId: largest.groupId, objectId: largest.objectId + 1n }
  )
  const payloads: Uint8Array[] = []
  for await (const object of objects) {
    if (object.objectPayloadLength > 0) {
      payloads.push(object.objectPayload)
    }
  }
  return payloads
}

/// The chat and the transcript of one replay namespace: the event timelines
/// (draft-ietf-moq-msf-01 §8) its catalog lists.
async function readEvents(session: MoqSession, namespace: string[], participant: string): Promise<TimelineEvent[]> {
  const events: TimelineEvent[] = []
  for (const { name, packaging, eventType } of extractCatalogTracks(await currentCatalog(session, namespace))) {
    const kind = packaging === 'eventtimeline' ? EVENT_KINDS[eventType ?? ''] : undefined
    if (kind) {
      for (const payload of await fetchTrack(session, namespace, name)) {
        events.push(...eventRecords(payload, kind, participant))
      }
    }
  }
  return events
}

/// Notes the capture timestamp of each review frame a player shows.
function shownFrames(onShown: (captureMicros: number) => void): DeliveryObserver {
  const ignore = () => {}
  return {
    label: ignore,
    opened: ignore,
    object: ignore,
    fetchObject: ignore,
    fetchFinished: ignore,
    forget: ignore,
    clearPlayhead: ignore,
    setPlayhead: ({ kind, captureMicros }) => {
      if (kind === 'fetch') {
        onShown(captureMicros)
      }
    }
  }
}

function tileFor(current: OpenedRecording, participant: string, content: string): Tile {
  const key = `${participant}/${content}`
  const existing = current.tiles.get(key)
  if (existing) {
    return existing
  }
  const tile = document.createElement('div')
  tile.className =
    'group relative aspect-video w-full overflow-hidden rounded-xl bg-slate-800 ring-2 ring-white/5 transition'
  tile.dataset.participant = participant
  tile.dataset.content = content
  const placeholder = document.createElement('div')
  placeholder.className = 'absolute inset-0 flex items-center justify-center'
  placeholder.append(avatar(participant, 'h-16 w-16 text-2xl'))
  const caption = document.createElement('div')
  caption.className =
    'pointer-events-none absolute bottom-9 left-1/2 hidden max-w-[90%] -translate-x-1/2 rounded-md bg-black/70 px-3 py-1 text-center text-sm text-white'
  const label = document.createElement('div')
  label.className =
    'pointer-events-none absolute bottom-2 left-2 max-w-[70%] truncate rounded-md bg-black/60 px-2 py-0.5 text-xs font-medium text-white'
  label.textContent = content === 'camera' ? participant : `${participant} · ${content}`
  const state = document.createElement('output')
  state.className =
    'pointer-events-none absolute bottom-2 right-2 max-w-[60%] truncate rounded bg-black/50 px-1.5 py-0.5 font-mono text-[10px] text-slate-200 opacity-0 transition group-hover:opacity-100'
  tile.append(placeholder, caption, label, state)
  const created: Tile = { participant, content, element: tile, caption, state, sources: [] }
  current.tiles.set(key, created)
  const keys = [...current.tiles.keys()].sort()
  element('videos').replaceChildren(...keys.map((sorted) => current.tiles.get(sorted)!.element))
  return created
}

async function addSource(current: OpenedRecording, namespace: string[]): Promise<void> {
  const key = namespace.join('/')
  if (current.sources.has(key)) {
    return
  }
  const rest = namespace.slice(current.recording.prefix.length)
  const participant = rest.slice(0, -2).join('/')
  const tile = tileFor(current, participant, rest.at(-2) ?? '')
  const container = document.createElement('div')
  container.className = 'source absolute inset-0'
  container.hidden = true
  container.dataset.namespace = key
  tile.caption.before(container)
  const player = new LivePlayer({
    container,
    callbacks: {
      onStateChange: requestRender,
      onLiveFrame: () => {},
      onLog: (level, message) => {
        if (level === 'error' || level === 'warn') {
          console[level](`[${key}] ${message}`)
        }
      }
    },
    deliveryObserver: shownFrames((captureMicros) => {
      source.shownMicros = captureMicros
    })
  })
  const source: Source = { player, container, shownMicros: undefined }
  current.sources.set(key, source)
  tile.sources.push(source)
  // Read the catalog for the events before the player starts, which follows the catalog from then
  // on, so that the session does not subscribe to it twice at once.
  const events = await readEvents(current.session, namespace, participant).catch((error: unknown) => {
    failureOf(current)(error)
    return []
  })
  if (opened !== current) {
    return
  }
  current.events = mergeEvents([...current.events, ...events])
  current.group.add(player)
  await player.start(current.session, namespace)
}

/// From the earliest start to the latest end of the sources' ranges; undefined
/// until one source has its range.
function recordingSpan(current: OpenedRecording): { startMicros: number; endMicros: number } | undefined {
  const axes = [...current.sources.values()].map(({ player }) => player.state.seek).filter((axis) => axis.seekable)
  if (axes.length === 0) {
    return undefined
  }
  return {
    startMicros: Math.min(...axes.map((axis) => axis.replayableStartSeconds)) * 1_000_000,
    endMicros: Math.max(...axes.map((axis) => axis.liveEdgeSeconds)) * 1_000_000
  }
}

function requestRender(): void {
  if (renderQueued) {
    return
  }
  renderQueued = true
  requestAnimationFrame(() => {
    renderQueued = false
    render()
  })
}

function render(): void {
  const current = opened
  if (!current) {
    return
  }
  const span = recordingSpan(current)
  const { paused, ended, positionMicros } = current.group.state
  const at = positionMicros ?? span?.startMicros
  play.disabled = seek.disabled = !span
  showPlaying(!paused)
  if (span) {
    seek.max = String((span.endMicros - span.startMicros) / 1_000_000)
    seek.dataset.startUs = String(span.startMicros)
    if (!seeking && at !== undefined) {
      seek.value = String((at - span.startMicros) / 1_000_000)
    }
    element('recorded-at').textContent = new Date(
      current.recording.startedAtMs + span.startMicros / 1000
    ).toLocaleString()
  }
  showPosition()
  const shown = renderTiles(current, at, span, positionMicros !== undefined)
  renderParticipants(current)
  renderEvents(current, at ?? Number.NEGATIVE_INFINITY)
  const loading = [...current.sources.values()].some(
    ({ player }) => !player.state.seek.seekable && player.state.catalogStatus.state !== 'error'
  )
  const buffering = !paused && shown.some(({ player }) => player.state.stalled)
  if (errorText) {
    showNotice(errorText, 'error')
  } else if (!span || buffering) {
    showNotice(span ? 'Buffering…' : 'Loading recording…', 'progress')
  } else {
    hideNotice()
  }
  status.value =
    errorText ??
    (ended
      ? 'Finished'
      : !span || loading
        ? 'Loading'
        : !current.started
          ? 'Ready'
          : paused
            ? 'Paused'
            : buffering
              ? 'Buffering'
              : 'Playing')
}

/// Whether the source plays at `at`: from its start to its end, or, for a
/// source that lasted until the recording's end, also past that end, keeping
/// its last frame as a media element does at its end.
function covers(source: Source, at: number, endMicros: number): boolean {
  const { seekable, replayableStartSeconds, liveEdgeSeconds } = source.player.state.seek
  const sourceEndMicros = liveEdgeSeconds * 1_000_000
  return (
    seekable &&
    at >= replayableStartSeconds * 1_000_000 &&
    (at <= sourceEndMicros || sourceEndMicros >= endMicros - RECORDING_END_SLACK_US)
  )
}

/// Shows each tile's source that plays at `at`, and returns the shown sources.
/// A screen share tile is visible only while one of its sources plays; a
/// camera tile is also visible, with its avatar, before the position is known.
function renderTiles(
  current: OpenedRecording,
  at: number | undefined,
  span: { endMicros: number } | undefined,
  positioned: boolean
): Source[] {
  const shown: Source[] = []
  let visible = 0
  for (const tile of current.tiles.values()) {
    const source =
      at === undefined || !span ? undefined : tile.sources.find((candidate) => covers(candidate, at, span.endMicros))
    for (const candidate of tile.sources) {
      candidate.container.hidden = candidate !== source
    }
    if (source) {
      shown.push(source)
    }
    if (source?.shownMicros !== undefined) {
      tile.element.dataset.captureUs = String(source.shownMicros)
    } else {
      delete tile.element.dataset.captureUs
    }
    tile.element.hidden = !source && (positioned || tile.content !== 'camera')
    visible += tile.element.hidden ? 0 : 1
    const failed = tile.sources.find(({ player }) => player.state.catalogStatus.state === 'error')?.player.state
    tile.state.value =
      source?.player.state.playbackStatus.text ?? failed?.catalogStatus.text ?? 'Not recorded at this time'
    const utterance =
      tile.content === 'camera' && at !== undefined ? captionAt(current.events, tile.participant, at) : undefined
    tile.caption.textContent = utterance?.text ?? ''
    tile.caption.classList.toggle('hidden', !utterance)
    tile.element.classList.toggle('ring-emerald-400', !!utterance)
    tile.element.classList.toggle('ring-white/5', !utterance)
  }
  element('videos').style.gridTemplateColumns = `repeat(${Math.ceil(Math.sqrt(visible))}, minmax(0, 1fr))`
  return shown
}

function renderParticipants(current: OpenedRecording): void {
  const participants = [...new Set([...current.tiles.values()].map(({ participant }) => participant))]
  const chips = element('member-chips')
  if (chips.dataset.participants === participants.join('\n')) {
    return
  }
  chips.dataset.participants = participants.join('\n')
  element('participants').textContent = `${participants.length} participants`
  chips.replaceChildren(
    ...participants.map((name) => {
      const chip = avatar(name, 'h-6 w-6 text-[11px]')
      chip.classList.add('ring-2', 'ring-slate-950')
      return chip
    })
  )
}

function renderEvents(current: OpenedRecording, at: number): void {
  for (const kind of ['chat', 'transcript']) {
    const visible = current.events.filter((event) => event.kind === kind && event.at <= at)
    const list = element<HTMLOListElement>(kind)
    const shown = visible.map((event) => `${event.at} ${event.text}`).join('\n')
    if (list.dataset.shown === shown) continue
    list.dataset.shown = shown
    list.replaceChildren(
      ...visible.map((event) => {
        const item = document.createElement('li')
        item.className = 'rounded-xl border border-white/10 bg-gray-900/70 px-4 py-3'
        item.dataset.timeUs = String(event.at)
        const heading = document.createElement('div')
        heading.className = 'flex items-center justify-between text-sm text-blue-200'
        const speaker = document.createElement('span')
        speaker.className = 'font-semibold'
        speaker.textContent = event.speaker
        const time = document.createElement('span')
        time.textContent = new Date(current.recording.startedAtMs + event.at / 1000).toLocaleTimeString()
        heading.append(speaker, time)
        const text = document.createElement('p')
        text.className = 'mt-2 whitespace-pre-wrap break-words text-base text-white'
        text.textContent = event.text
        item.append(heading, text)
        return item
      })
    )
    list.scrollTop = list.scrollHeight
  }
}

function showRoom(prefix: string[]): void {
  element('room-name').textContent = prefix.at(-3) ?? 'Recording'
  element('recorded-at').textContent = ''
  element('participants').textContent = ''
  element('member-chips').replaceChildren()
  delete element('member-chips').dataset.participants
}

async function closeRecording(): Promise<void> {
  const current = opened
  opened = undefined
  if (!current) {
    return
  }
  current.group.pause()
  current.recording.close()
  await Promise.allSettled([...current.sources.values()].map(({ player }) => player.stop()))
  current.session.close()
}

async function open(): Promise<void> {
  load.disabled = true
  openError.classList.add('hidden')
  await closeRecording()
  errorText = undefined
  hideNotice()
  element('videos').replaceChildren()
  for (const kind of ['chat', 'transcript']) {
    element(kind).replaceChildren()
    delete element(kind).dataset.shown
  }
  play.disabled = seek.disabled = true
  seek.max = seek.value = '0'
  showPosition()
  showPlaying(false)
  status.value = 'Opening recording'
  const accessToken = token.value || undefined
  let session: MoqSession | undefined
  try {
    session = await MoqSession.connect(relay.value, { token: accessToken, versions: offeredVersions() })
    const replay = await openRecording(session, recorder.value, recording.value, { token: accessToken })
    const watch = await session.watchNamespaces(replay.prefix)
    const group = new PlaybackGroup({ onStateChange: requestRender })
    group.pause()
    const current: OpenedRecording = {
      session,
      recording: replay,
      group,
      tiles: new Map(),
      sources: new Map(),
      events: [],
      started: false
    }
    opened = current
    showRoom(replay.prefix)
    openDialog.classList.add('hidden')
    const failed = failureOf(current)
    void replay.closed.then((error) => error && failed(error))
    void (async () => {
      for await (const { kind, namespace } of watch) {
        if (kind === 'announced') {
          void addSource(current, namespace).catch(failed)
        }
      }
    })().catch(failed)
    requestRender()
  } catch (error) {
    session?.close()
    failure(error)
    openError.textContent = status.value
    openError.classList.remove('hidden')
  } finally {
    load.disabled = false
  }
}

element<HTMLFormElement>('open').addEventListener('submit', (event) => {
  event.preventDefault()
  void open()
})
play.addEventListener('click', () => {
  const current = opened
  const span = current && recordingSpan(current)
  if (!current || !span) {
    return
  }
  if (!current.group.state.paused) {
    current.group.pause()
    return
  }
  if (!current.started) {
    current.started = true
    current.group.seek(span.startMicros)
  }
  current.group.play()
})
seek.addEventListener('input', () => {
  seeking = true
  showPosition()
})
seek.addEventListener('change', () => {
  seeking = false
  const current = opened
  const span = current && recordingSpan(current)
  if (!current || !span) {
    return
  }
  current.started = true
  current.group.seek(span.startMicros + Number(seek.value) * 1_000_000)
  requestRender()
})
element('reopen').addEventListener('click', () => openDialog.classList.remove('hidden'))
openDialog.addEventListener('click', (event) => {
  if (event.target === openDialog && opened) openDialog.classList.add('hidden')
})
element('toggle-sidebar').addEventListener('click', (event) => {
  const open = sidebar.classList.toggle('hidden') === false
  ;(event.currentTarget as HTMLElement).ariaPressed = String(open)
})
for (const tab of document.querySelectorAll<HTMLButtonElement>('.sidebar-tab')) {
  tab.addEventListener('click', () => {
    for (const other of document.querySelectorAll<HTMLButtonElement>('.sidebar-tab')) {
      const selected = other === tab
      other.ariaSelected = String(selected)
      element(other.dataset.tab!).classList.toggle('hidden', !selected)
    }
  })
}
window.addEventListener('pagehide', () => {
  opened?.recording.close()
  opened?.session.close()
})
