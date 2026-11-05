import { MoqSession, type ReceivedObject } from 'moq-wasm'
import { bearerToken } from '../../utils/access'
import { extractCatalogTracks, type MediaCatalogTrack } from 'moq-wasm/media/catalog'
import { readLocHeader } from 'moq-wasm/media/internal/loc'
import { VideoPlayout } from 'moq-wasm/media/player/videoPlayout'
import {
  eventRecords,
  keyframeDetector,
  mergeEvents,
  chunkIndexAt,
  type TimelineEvent,
  type IndexedChunk
} from './timeline'
import { startPageTelemetry } from '../../utils/pageTelemetry'
import { setUpRelayUrlControls } from '../../utils/relayPresets'

type Track = {
  namespace: string
  source_namespace: string
  content: string
  name: string
  chunks: IndexedChunk[]
}
type Frame = { at: number; data: Uint8Array; key: boolean; groupId: bigint; objectId: bigint }
type Media = {
  descriptor: Track
  config: MediaCatalogTrack
  chunks: IndexedChunk[]
  canvas?: HTMLCanvasElement
  tile?: HTMLElement
  caption?: HTMLElement
  enabled?: HTMLInputElement
  playout?: VideoPlayout
  state?: HTMLOutputElement
}
const element = <T extends HTMLElement>(id: string) => document.getElementById(id) as T
const status = element<HTMLOutputElement>('status')
const play = element<HTMLButtonElement>('play')
const seek = element<HTMLInputElement>('seek')
const position = element<HTMLOutputElement>('position')
const recorder = element<HTMLInputElement>('recorder')
const relay = element<HTMLInputElement>('relay')
const recording = element<HTMLInputElement>('recording')
const token = element<HTMLInputElement>('token')
const openDialog = element('open-dialog')
const openError = element('open-error')
const sidebar = element('sidebar')
const params = new URLSearchParams(location.search)
recorder.value = params.get('recorder') ?? recorder.value
recording.value = params.get('id') ?? ''
token.value ||= bearerToken() ?? ''
setUpRelayUrlControls(relay, element('relayPresets'))
let session: MoqSession | undefined
startPageTelemetry('recording-player', () => relay.value)
/// The recorder keeps a replay running while a `publish` answer to it is open.
type Lease = {
  replay: { tracks: Track[]; namespace_prefix: string; renew_after_seconds: number }
  reader: ReadableStreamDefaultReader<Uint8Array>
}
let lease: Lease | undefined
let renewal: ReturnType<typeof setTimeout> | undefined
let publish: { url: string; headers: HeadersInit } | undefined
let generation = 0
let media: Media[] = []
let events: TimelineEvent[] = []
let origin = 0
let end = 0
let playing = false
let target = 0
let anchorMs = 0
let audioContext: AudioContext | undefined
let audioAnchor = 0
let tick: ReturnType<typeof setInterval> | undefined
const sources = new Set<AudioBufferSourceNode>()
const decoders = new Set<VideoDecoder | AudioDecoder>()
const fetchWaiters: (() => void)[] = []
const idleWaiters: (() => void)[] = []
let activeFetches = 0

async function acquireFetch(): Promise<() => void> {
  if (activeFetches < 3) activeFetches++
  else await new Promise<void>((resolve) => fetchWaiters.push(resolve))
  return () => {
    const next = fetchWaiters.shift()
    if (next) next()
    else if (--activeFetches === 0) for (const waiter of idleWaiters.splice(0)) waiter()
  }
}

function whenFetchesIdle(action: () => void): void {
  if (activeFetches === 0) action()
  else idleWaiters.push(action)
}

async function openLease(): Promise<Lease> {
  const response = await fetch(publish!.url, { method: 'POST', headers: publish!.headers })
  if (!response.ok) {
    const body = await response.json().catch(() => ({}))
    throw new Error(body.error ?? `Recorder returned ${response.status}`)
  }
  const reader = response.body!.getReader()
  const decoder = new TextDecoder()
  let text = ''
  while (!text.includes('\n')) {
    const { value, done } = await reader.read()
    if (done) throw new Error('The recorder closed the replay')
    text += decoder.decode(value, { stream: true })
  }
  return { replay: JSON.parse(text.slice(0, text.indexOf('\n'))), reader }
}

/// Reads `held` until it ends; if it is still the current lease then, takes a new one.
async function hold(held: Lease): Promise<void> {
  try {
    while (!(await held.reader.read()).done);
  } catch {
    // A cancelled or broken answer counts as ended.
  }
  if (held === lease) await renew().catch(failure)
}

/// Takes a new lease, then cancels the previous one once no FETCH is in flight:
/// the relay sends new FETCHes to the newest replay first, so a replay that only
/// the previous lease kept can end without cutting a FETCH off.
async function renew(): Promise<void> {
  clearTimeout(renewal)
  const next = await openLease()
  const previous = lease
  lease = next
  void hold(next)
  renewal = setTimeout(() => void renew().catch(failure), next.replay.renew_after_seconds * 1000)
  if (previous) whenFetchesIdle(() => void previous.reader.cancel())
}

function releaseLease(): void {
  clearTimeout(renewal)
  const held = lease
  lease = undefined
  void held?.reader.cancel()
}

/// How long a transcribed utterance stays on its speaker's tile.
const CAPTION_US = 4_000_000

const memberName = (track: Track) => track.source_namespace.split('/').at(-1)!

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

function showRoom(prefix: string): void {
  element('room-name').textContent = prefix.split('/recordings/')[0].split('/').at(-1) || 'Recording'
  element('recorded-at').textContent = new Date(origin / 1000).toLocaleString()
  const members = [...new Set(media.map((track) => memberName(track.descriptor)))]
  element('participants').textContent = `${members.length} participants`
  element('member-chips').replaceChildren(
    ...members.map((name) => {
      const chip = avatar(name, 'h-6 w-6 text-[11px]')
      chip.classList.add('ring-2', 'ring-slate-950')
      return chip
    })
  )
  const tiles = media.filter((track) => track.tile).length
  element('videos').style.gridTemplateColumns = `repeat(${Math.ceil(Math.sqrt(tiles))}, minmax(0, 1fr))`
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
  status.value = error instanceof Error ? error.message : String(error)
  showNotice(status.value, 'error')
}
function stop(): void {
  generation++
  playing = false
  clearInterval(tick)
  tick = undefined
  for (const decoder of decoders) if (decoder.state !== 'closed') decoder.close()
  decoders.clear()
  for (const source of sources) {
    source.onended = null
    source.stop()
  }
  sources.clear()
  for (const track of media) track.playout?.flush()
  showPlaying(false)
  hideNotice()
}

async function readRange(
  track: Track,
  range: IndexedChunk,
  consume: (object: ReceivedObject) => void,
  current = generation
): Promise<void> {
  const connected = session!
  const release = await acquireFetch()
  try {
    if (current !== generation || !range.first || !range.last) return
    const stream = await connected.fetch(
      track.namespace.split('/'),
      track.name,
      { groupId: BigInt(range.first.group), objectId: BigInt(range.first.object) },
      { groupId: BigInt(range.last.group), objectId: BigInt(range.last.object) + 1n }
    )
    for await (const object of stream) {
      if (current !== generation) break
      if (object.objectPayloadLength) consume(object)
      else if (object.objectStatus === 1) throw new Error('The recording contains a missing object')
    }
  } finally {
    release()
  }
}

async function readTrack(track: Track, consume: (object: ReceivedObject) => void): Promise<void> {
  for (const chunk of track.chunks) await readRange(track, chunk, consume)
}

async function loadFrames(
  track: Media,
  chunk: number,
  detectKeyframe: (data: Uint8Array) => boolean,
  current: number
): Promise<Frame[]> {
  const frames: Frame[] = []
  await readRange(
    track.descriptor,
    track.chunks[chunk],
    (object) => {
      const at = readLocHeader(object.locHeader).captureTimestampMicros
      if (at === undefined || !Number.isSafeInteger(at))
        throw new Error(`No capture clock for ${track.descriptor.name}`)
      frames.push({
        at,
        data: object.objectPayload,
        key: track.config.role !== 'video' || detectKeyframe(object.objectPayload),
        groupId: object.groupId,
        objectId: object.objectId
      })
    },
    current
  )
  return frames
}

type PreparedTrack = {
  track: Media
  chunk: number
  frames: Frame[]
  startIndex: number
  detectKeyframe: (data: Uint8Array) => boolean
}

async function prepareTrack(track: Media, current: number, at: number): Promise<PreparedTrack | undefined> {
  if (track.enabled && !track.enabled.checked) {
    track.state!.value = 'Hidden'
    return
  }
  const detectKeyframe = keyframeDetector(track.config.codec!)
  let chunk = chunkIndexAt(track.chunks, at)
  const requestedChunk = chunk
  if (track.config.role !== 'video') {
    const frames = await loadFrames(track, chunk, detectKeyframe, current)
    return {
      track,
      chunk,
      frames,
      startIndex: Math.max(
        0,
        frames.findIndex((frame) => frame.at >= at - 100_000)
      ),
      detectKeyframe
    }
  }
  for (; chunk >= 0 && current === generation; chunk--) {
    const frames = await loadFrames(track, chunk, detectKeyframe, current)
    let startIndex = frames.findLastIndex((frame) => frame.key && frame.at <= at)
    if (startIndex < 0 && chunk === 0) startIndex = frames.findIndex((frame) => frame.key)
    if (startIndex >= 0) return { track, chunk, frames, startIndex, detectKeyframe }
  }
  for (chunk = requestedChunk + 1; chunk < track.chunks.length && current === generation; chunk++) {
    const frames = await loadFrames(track, chunk, detectKeyframe, current)
    const startIndex = frames.findIndex((frame) => frame.key)
    if (startIndex >= 0) return { track, chunk, frames, startIndex, detectKeyframe }
  }
  if (track.state) track.state.value = 'No decodable access point'
}

function addVideo(track: Media): void {
  const name = memberName(track.descriptor)
  const tile = document.createElement('div')
  tile.className =
    'group relative aspect-video w-full overflow-hidden rounded-xl bg-slate-800 ring-2 ring-white/5 transition'
  const placeholder = document.createElement('div')
  placeholder.className = 'absolute inset-0 flex items-center justify-center'
  placeholder.append(avatar(name, 'h-16 w-16 text-2xl'))
  const canvas = document.createElement('canvas')
  canvas.className = 'absolute inset-0 h-full w-full object-contain'
  canvas.width = track.config.width ?? 640
  canvas.height = track.config.height ?? 360
  canvas.dataset.source = track.descriptor.source_namespace
  canvas.dataset.content = track.descriptor.content
  const caption = document.createElement('div')
  caption.className =
    'pointer-events-none absolute bottom-9 left-1/2 hidden max-w-[90%] -translate-x-1/2 rounded-md bg-black/70 px-3 py-1 text-center text-sm text-white'
  const label = document.createElement('div')
  label.className =
    'pointer-events-none absolute bottom-2 left-2 max-w-[70%] truncate rounded-md bg-black/60 px-2 py-0.5 text-xs font-medium text-white'
  label.textContent = track.descriptor.content === 'camera' ? name : `${name} · ${track.descriptor.content}`
  const state = document.createElement('output')
  state.className =
    'pointer-events-none absolute bottom-2 right-2 max-w-[60%] truncate rounded bg-black/50 px-1.5 py-0.5 font-mono text-[10px] text-slate-200 opacity-0 transition group-hover:opacity-100'
  const toggle = document.createElement('label')
  toggle.className =
    'absolute right-2 top-2 flex cursor-pointer items-center gap-1 rounded-full bg-black/60 px-2 py-1 text-xs text-white opacity-0 transition focus-within:opacity-100 group-hover:opacity-100'
  const enabled = document.createElement('input')
  enabled.type = 'checkbox'
  enabled.className = 'accent-blue-500'
  enabled.checked = media.filter((entry) => entry.canvas).length < 8
  toggle.append(enabled, 'Show')
  tile.append(placeholder, canvas, caption, label, state, toggle)
  element('videos').append(tile)
  track.canvas = canvas
  track.tile = tile
  track.caption = caption
  track.enabled = enabled
  track.state = state
  track.playout = new VideoPlayout((frame) => {
    canvas.width = frame.displayWidth
    canvas.height = frame.displayHeight
    canvas.getContext('2d')!.drawImage(frame, 0, 0)
    canvas.dataset.captureUs = String(frame.timestamp)
    state.value = 'Playing'
    frame.close()
  })
  enabled.addEventListener('change', () => {
    if (playing) void start().catch(failure)
  })
}

element<HTMLFormElement>('open').addEventListener('submit', async (event) => {
  event.preventDefault()
  stop()
  releaseLease()
  session?.close()
  media = []
  events = []
  element('videos').replaceChildren()
  element('chat').replaceChildren()
  element('transcript').replaceChildren()
  delete element('chat').dataset.position
  delete element('transcript').dataset.position
  play.disabled = seek.disabled = true
  openError.classList.add('hidden')
  element<HTMLButtonElement>('load').disabled = true
  status.value = 'Opening recording'
  try {
    publish = {
      url: `${recorder.value.replace(/\/$/, '')}/recordings/${encodeURIComponent(recording.value)}/publish`,
      headers: token.value ? { Authorization: `Bearer ${token.value}` } : {}
    }
    await renew()
    const replay = lease!.replay
    session = await MoqSession.connect(relay.value, { token: token.value || undefined })
    const tracks = replay.tracks
    const catalogs = new Map<string, MediaCatalogTrack[]>()
    for (const track of tracks.filter((track) => track.name === 'catalog')) {
      const definitions: MediaCatalogTrack[] = []
      await readTrack(track, (object) =>
        definitions.push(...extractCatalogTracks(JSON.parse(new TextDecoder().decode(object.objectPayload))))
      )
      catalogs.set(track.namespace, definitions)
    }
    for (const descriptor of tracks.filter((track) => track.name !== 'catalog')) {
      status.value = `Reading ${descriptor.source_namespace} / ${descriptor.name}`
      if (descriptor.name === 'chat' || descriptor.name === 'transcript') {
        await readTrack(descriptor, (object) =>
          events.push(...eventRecords(object.objectPayload, descriptor.source_namespace, descriptor.name))
        )
        continue
      }
      const definitions = catalogs.get(descriptor.namespace)?.filter((track) => track.name === descriptor.name) ?? []
      const config = definitions.at(-1)
      if (!config?.codec || !['video', 'audio'].includes(config.role ?? ''))
        throw new Error(`No media configuration for ${descriptor.name}`)
      if (
        definitions.some(
          (track) =>
            track.codec !== config.codec || track.samplerate !== config.samplerate || track.initData !== config.initData
        )
      ) {
        throw new Error(`Configuration changes in ${descriptor.name} require segmented playback`)
      }
      if (
        descriptor.chunks.some(
          (chunk) => BigInt(chunk.data_size) > 0n && (chunk.capture_start_us === null || chunk.capture_end_us === null)
        )
      )
        throw new Error(`No capture clock for ${descriptor.name}`)
      const chunks = descriptor.chunks
        .filter(
          (chunk) => chunk.first && chunk.last && chunk.capture_start_us !== null && chunk.capture_end_us !== null
        )
        .sort((a, b) =>
          BigInt(a.first!.group) < BigInt(b.first!.group)
            ? -1
            : BigInt(a.first!.group) > BigInt(b.first!.group)
              ? 1
              : Number(BigInt(a.first!.object) - BigInt(b.first!.object))
        )
      if (!chunks.length) continue
      for (const chunk of chunks)
        if (
          !Number.isSafeInteger(Number(chunk.capture_start_us)) ||
          !Number.isSafeInteger(Number(chunk.capture_end_us))
        )
          throw new Error(`Invalid capture clock for ${descriptor.name}`)
      const track: Media = { descriptor, config, chunks }
      if (config.role === 'video') addVideo(track)
      media.push(track)
    }
    events = mergeEvents(events)
    if (!media.length) throw new Error('No recorded media with source timestamps')
    origin = media.reduce(
      (earliest, track) =>
        track.chunks.reduce((value, chunk) => Math.min(value, Number(chunk.capture_start_us)), earliest),
      Infinity
    )
    end = media.reduce(
      (latest, track) => track.chunks.reduce((value, chunk) => Math.max(value, Number(chunk.capture_end_us)), latest),
      -Infinity
    )
    seek.max = String((end - origin) / 1_000_000)
    seek.value = '0'
    showPosition()
    showRoom(replay.namespace_prefix)
    renderEvents(origin)
    status.value = 'Ready'
    openDialog.classList.add('hidden')
    play.disabled = seek.disabled = false
  } catch (error) {
    releaseLease()
    session?.close()
    failure(error)
    openError.textContent = status.value
    openError.classList.remove('hidden')
  } finally {
    element<HTMLButtonElement>('load').disabled = false
  }
})

function renderEvents(at: number): void {
  for (const kind of ['chat', 'transcript']) {
    const visible = events.filter((event) => event.kind === kind && event.at <= at)
    const list = element<HTMLOListElement>(kind)
    if (list.dataset.position === String(visible.length)) continue
    list.dataset.position = String(visible.length)
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
        time.textContent = new Date(event.at / 1000).toLocaleTimeString()
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
  for (const track of media) {
    if (!track.tile || !track.caption) continue
    const name = memberName(track.descriptor)
    const utterance = events.findLast(
      (event) => event.kind === 'transcript' && event.speaker === name && event.at <= at && at - event.at < CAPTION_US
    )
    track.caption.textContent = utterance?.text ?? ''
    track.caption.classList.toggle('hidden', !utterance)
    track.tile.classList.toggle('ring-emerald-400', !!utterance)
    track.tile.classList.toggle('ring-white/5', !utterance)
  }
}

function playAudio(data: AudioData, current: number): void {
  try {
    if (generation !== current || !audioContext) return
    const at = audioAnchor + (data.timestamp - target) / 1_000_000
    const offset = Math.max(0, audioContext.currentTime - at)
    const duration = data.numberOfFrames / data.sampleRate
    if (offset >= duration) return
    const buffer = audioContext.createBuffer(data.numberOfChannels, data.numberOfFrames, data.sampleRate)
    for (let channel = 0; channel < data.numberOfChannels; channel++)
      data.copyTo(buffer.getChannelData(channel), { planeIndex: channel, format: 'f32-planar' })
    const source = audioContext.createBufferSource()
    source.buffer = buffer
    source.connect(audioContext.destination)
    source.onended = () => sources.delete(source)
    sources.add(source)
    source.start(Math.max(audioContext.currentTime, at), offset)
  } finally {
    data.close()
  }
}

async function decodeTrack(prepared: PreparedTrack, current: number): Promise<void> {
  const { track, detectKeyframe } = prepared
  const onError = (error: DOMException) => {
    if (track.state) track.state.value = error.message
    failure(error)
  }
  const decoder =
    track.config.role === 'video'
      ? new VideoDecoder({
          output: (frame) => {
            if (current !== generation || frame.timestamp < target) {
              frame.close()
              return
            }
            track.playout!.present(frame, anchorMs + (frame.timestamp - target) / 1000)
          },
          error: onError
        })
      : new AudioDecoder({ output: (data) => playAudio(data, current), error: onError })
  decoders.add(decoder)
  try {
    if (decoder instanceof VideoDecoder) decoder.configure({ codec: track.config.codec! })
    else
      decoder.configure({
        codec: track.config.codec!,
        sampleRate: track.config.samplerate!,
        numberOfChannels: track.config.channelConfig === 'mono' ? 1 : 2,
        description: track.config.initData
          ? Uint8Array.from(atob(track.config.initData), (value) => value.charCodeAt(0))
          : undefined
      })
    let frames = prepared.frames
    prepared.frames = []
    let startIndex = prepared.startIndex
    let last: { groupId: bigint; objectId: bigint } | undefined
    for (let chunk = prepared.chunk; chunk < track.chunks.length && current === generation; chunk++) {
      if (chunk !== prepared.chunk) {
        const end = track.chunks[chunk].last!
        if (
          last &&
          (BigInt(end.group) < last.groupId ||
            (BigInt(end.group) === last.groupId && BigInt(end.object) <= last.objectId))
        )
          continue
        frames = await loadFrames(track, chunk, detectKeyframe, current)
        startIndex = 0
      }
      for (let index = startIndex; index < frames.length && current === generation; index++) {
        const frame = frames[index]
        if (
          last &&
          (frame.groupId < last.groupId || (frame.groupId === last.groupId && frame.objectId <= last.objectId))
        )
          continue
        while (
          current === generation &&
          (frame.at > target + (performance.now() - anchorMs + 2000) * 1000 ||
            decoder.decodeQueueSize + (track.playout?.queued ?? 0) > 30)
        ) {
          await new Promise((resolve) => setTimeout(resolve, 20))
        }
        if (current !== generation || decoder.state === 'closed') return
        if (decoder instanceof VideoDecoder)
          decoder.decode(
            new EncodedVideoChunk({ type: frame.key ? 'key' : 'delta', timestamp: frame.at, data: frame.data })
          )
        else decoder.decode(new EncodedAudioChunk({ type: 'key', timestamp: frame.at, data: frame.data }))
        last = frame
      }
      frames = []
    }
    if (decoder.state !== 'closed') await decoder.flush()
  } finally {
    decoders.delete(decoder)
    if (decoder.state !== 'closed') decoder.close()
  }
}

async function start(): Promise<void> {
  stop()
  const current = generation
  target = origin + Number(seek.value) * 1_000_000
  audioContext ??= new AudioContext()
  await audioContext.resume()
  if (generation !== current) return
  playing = true
  showPlaying(true)
  status.value = 'Buffering'
  showNotice('Buffering…', 'progress')
  const prepared: PreparedTrack[] = []
  for (const track of media) {
    const input = await prepareTrack(track, current, target)
    if (generation !== current) return
    if (input) prepared.push(input)
  }
  anchorMs = performance.now() + 200
  audioAnchor = audioContext.currentTime + 0.2
  for (const track of media) {
    if (track.canvas) {
      track.canvas.getContext('2d')!.clearRect(0, 0, track.canvas.width, track.canvas.height)
      delete track.canvas.dataset.captureUs
    }
  }
  showPlaying(true)
  status.value = 'Playing'
  hideNotice()
  tick = setInterval(() => {
    const at = Math.min(end, target + Math.max(0, performance.now() - anchorMs) * 1000)
    seek.value = String((at - origin) / 1_000_000)
    showPosition()
    renderEvents(at)
    for (const track of media) {
      if (track.canvas && track.enabled?.checked && at > Number(track.canvas.dataset.captureUs ?? at) + 1_000_000)
        track.state!.value = 'No recorded video at this time'
    }
    if (at >= end) {
      stop()
      status.value = 'Finished'
    }
  }, 50)
  await Promise.all(
    prepared.map((input) =>
      decodeTrack(input, current).catch((error) => {
        if (generation === current) {
          if (input.track.state) input.track.state.value = String(error)
          failure(error)
        }
      })
    )
  )
}

play.addEventListener('click', () => {
  if (playing) stop()
  else void start().catch(failure)
})
seek.addEventListener('input', showPosition)
seek.addEventListener('change', () => {
  renderEvents(origin + Number(seek.value) * 1_000_000)
  void start().catch(failure)
})
element('reopen').addEventListener('click', () => openDialog.classList.remove('hidden'))
openDialog.addEventListener('click', (event) => {
  if (event.target === openDialog && media.length) openDialog.classList.add('hidden')
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
  stop()
  releaseLease()
  session?.close()
  void audioContext?.close()
})
