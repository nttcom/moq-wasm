import { MoqSession, type MoqSubscription, type ReceivedObject } from 'moq-wasm'
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
const recording = element<HTMLInputElement>('recording')
const token = element<HTMLInputElement>('token')
const params = new URLSearchParams(location.search)
recorder.value = params.get('recorder') ?? recorder.value
recording.value = params.get('id') ?? ''
let session: MoqSession | undefined
let presence: MoqSubscription | undefined
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
let activeFetches = 0

async function acquireFetch(): Promise<() => void> {
  if (activeFetches < 3) activeFetches++
  else await new Promise<void>((resolve) => fetchWaiters.push(resolve))
  return () => {
    const next = fetchWaiters.shift()
    if (next) next()
    else activeFetches--
  }
}

function failure(error: unknown): void {
  status.value = error instanceof Error ? error.message : String(error)
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
  play.textContent = 'Play'
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
  const figure = document.createElement('figure')
  const caption = document.createElement('figcaption')
  const label = document.createElement('label')
  const enabled = document.createElement('input')
  enabled.type = 'checkbox'
  enabled.checked = media.filter((entry) => entry.canvas).length < 8
  label.append(enabled, ` ${track.descriptor.source_namespace.split('/').at(-1)} · ${track.descriptor.content}`)
  const state = document.createElement('output')
  caption.append(label, state)
  const canvas = document.createElement('canvas')
  canvas.width = track.config.width ?? 640
  canvas.height = track.config.height ?? 360
  canvas.dataset.source = track.descriptor.source_namespace
  canvas.dataset.content = track.descriptor.content
  figure.append(caption, canvas)
  element('videos').append(figure)
  track.canvas = canvas
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
  await presence?.close().catch(() => undefined)
  presence = undefined
  session?.close()
  media = []
  events = []
  element('videos').replaceChildren()
  element('chat').replaceChildren()
  element('transcript').replaceChildren()
  delete element('chat').dataset.position
  delete element('transcript').dataset.position
  play.disabled = seek.disabled = true
  element<HTMLButtonElement>('load').disabled = true
  status.value = 'Opening recording'
  try {
    const response = await fetch(
      `${recorder.value.replace(/\/$/, '')}/recordings/${encodeURIComponent(recording.value)}/publish`,
      {
        method: 'POST',
        headers: token.value ? { Authorization: `Bearer ${token.value}` } : {}
      }
    )
    const replay = await response.json()
    if (!response.ok) throw new Error(replay.error ?? `Recorder returned ${response.status}`)
    session = await MoqSession.connect(replay.relay, { token: token.value || undefined })
    const tracks: Track[] = replay.tracks
    const presenceTrack = tracks.find((track) => track.name === 'catalog') ?? tracks[0]
    if (!presenceTrack) throw new Error('Recording has no tracks')
    presence = await session.subscribe(presenceTrack.namespace.split('/'), presenceTrack.name, { forward: false })
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
    renderEvents(origin)
    status.value = 'Ready'
    play.disabled = seek.disabled = false
  } catch (error) {
    session?.close()
    failure(error)
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
        item.textContent = `${event.speaker}: ${event.text}`
        item.dataset.timeUs = String(event.at)
        return item
      })
    )
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
  play.textContent = 'Pause'
  status.value = 'Buffering'
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
  play.textContent = 'Pause'
  status.value = 'Playing'
  tick = setInterval(() => {
    const at = Math.min(end, target + Math.max(0, performance.now() - anchorMs) * 1000)
    seek.value = String((at - origin) / 1_000_000)
    position.value = `${Number(seek.value).toFixed(1)} s`
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
seek.addEventListener('change', () => {
  renderEvents(origin + Number(seek.value) * 1_000_000)
  void start().catch(failure)
})
window.addEventListener('pagehide', () => {
  stop()
  void presence?.close().catch(() => undefined)
  session?.close()
  void audioContext?.close()
})
