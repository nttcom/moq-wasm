import { extractCatalogTracks, type MediaCatalogTrack } from 'moq-wasm/media/catalog'
import { LocReview } from 'moq-wasm/media/player/locReview'
import type { ReviewFrame } from 'moq-wasm/media/player/rewind'

type ArchiveObject = {
  track?: string
  group?: string
  object?: string
  capture_us?: string | null
  payload?: string
  complete?: boolean
  version?: number
}

type Entry = { offset: number; length: number; groupId: bigint; objectId: bigint; captureMicros: number }

const file = document.querySelector<HTMLInputElement>('#file')!
const play = document.querySelector<HTMLButtonElement>('#play')!
const seek = document.querySelector<HTMLInputElement>('#seek')!
const status = document.querySelector<HTMLOutputElement>('#status')!
const canvas = document.querySelector<HTMLCanvasElement>('#video')!
let generation = 0
let source: File | undefined
let video: Entry[] = []
let audio: Entry[] = []
let videoTrack: MediaCatalogTrack | undefined
let audioTrack: MediaCatalogTrack | undefined
let origin = 0
const review = new LocReview(canvas, {
  onFrameShown: (at) => {
    seek.value = String((at - origin) / 1_000_000)
    canvas.dataset.captureUs = String(at)
  },
  onLog: (_level, message) => {
    status.value = message
  }
})

function decode(value: string): Uint8Array {
  return Uint8Array.from(atob(value), (c) => c.charCodeAt(0))
}

async function indexArchive(selected: File): Promise<{ entries: Map<string, Entry[]>; tracks: MediaCatalogTrack[] }> {
  const entries = new Map<string, Entry[]>()
  const tracks: MediaCatalogTrack[] = []
  const reader = selected.stream().getReader()
  const decoder = new TextDecoder()
  let pending = new Uint8Array(0)
  let offset = 0
  let first = true
  let complete = false
  while (true) {
    const { value, done } = await reader.read()
    if (done) break
    const bytes = new Uint8Array(pending.length + value.length)
    bytes.set(pending)
    bytes.set(value, pending.length)
    let start = 0
    for (let i = 0; i < bytes.length; i++) {
      if (bytes[i] !== 10) continue
      const length = i - start
      const object: ArchiveObject = JSON.parse(decoder.decode(bytes.subarray(start, i)))
      if (first && object.version !== 1) throw new Error('Unsupported archive')
      first = false
      complete = object.complete === true
      if (object.track === 'catalog' && object.payload) {
        tracks.push(...extractCatalogTracks(JSON.parse(decoder.decode(decode(object.payload)))))
      } else if (
        object.track &&
        object.group !== undefined &&
        object.object !== undefined &&
        object.capture_us &&
        object.payload
      ) {
        const captureMicros = Number(object.capture_us)
        if (!Number.isSafeInteger(captureMicros)) throw new Error('Invalid capture timestamp')
        const trackEntries = entries.get(object.track) ?? []
        trackEntries.push({
          offset,
          length,
          groupId: BigInt(object.group),
          objectId: BigInt(object.object),
          captureMicros
        })
        entries.set(object.track, trackEntries)
      }
      offset += length + 1
      start = i + 1
    }
    pending = bytes.slice(start)
  }
  if (pending.length || !complete) throw new Error('Archive is incomplete')
  for (const track of entries.values()) track.sort((a, b) => a.captureMicros - b.captureMicros)
  return { entries, tracks }
}

async function readFrames(selected: File, entries: Entry[]): Promise<ReviewFrame[]> {
  return Promise.all(
    entries.map(async (entry) => {
      const object: ArchiveObject = JSON.parse(await selected.slice(entry.offset, entry.offset + entry.length).text())
      if (!object.payload) throw new Error('Archive object has no payload')
      return { ...entry, data: decode(object.payload) }
    })
  )
}

function lowerBound(entries: Entry[], timestamp: number): number {
  let first = 0
  let last = entries.length
  while (first < last) {
    const middle = first + Math.floor((last - first) / 2)
    if (entries[middle].captureMicros < timestamp) first = middle + 1
    else last = middle
  }
  return first
}

file.addEventListener('change', async () => {
  const current = ++generation
  review.playout.stop()
  play.disabled = seek.disabled = true
  try {
    const selected = file.files?.[0]
    if (!selected) return
    status.value = 'Reading archive'
    const { entries, tracks } = await indexArchive(selected)
    if (current !== generation) return
    videoTrack = tracks.find((track) => track.role === 'video' && entries.has(track.name))
    audioTrack = tracks.find((track) => track.role === 'audio' && entries.has(track.name))
    if (!videoTrack?.codec) throw new Error('No playable video track')
    source = selected
    video = entries.get(videoTrack.name) ?? []
    audio = audioTrack ? (entries.get(audioTrack.name) ?? []) : []
    if (!video.length) throw new Error('Video has no capture timestamps')
    origin = video[0].captureMicros
    seek.max = String((video[video.length - 1].captureMicros - origin) / 1_000_000)
    seek.value = '0'
    play.disabled = seek.disabled = false
    status.value = 'Ready'
  } catch (error) {
    if (current === generation) status.value = error instanceof Error ? error.message : String(error)
  }
})

async function start(): Promise<void> {
  if (!videoTrack?.codec || !source) return
  const current = ++generation
  const selected = source
  const target = origin + Number(seek.value) * 1_000_000
  const startIndex = video.findLastIndex((frame) => frame.objectId === 0n && frame.captureMicros <= target)
  if (startIndex < 0) {
    status.value = 'No keyframe before this position'
    return
  }
  const audioConfig =
    audioTrack?.codec && audioTrack.samplerate
      ? {
          codec: audioTrack.codec,
          sampleRate: audioTrack.samplerate,
          numberOfChannels: audioTrack.channelConfig === 'mono' ? 1 : 2,
          description: audioTrack.initData ? decode(audioTrack.initData) : undefined
        }
      : undefined
  review.playout.start(target)
  status.value = 'Playing'
  try {
    let index = startIndex
    while (index < video.length && current === generation) {
      const from = video[index].captureMicros
      let end = index + 1
      while (end < video.length && (video[end].objectId !== 0n || video[end].captureMicros < from + 5_000_000)) end++
      const until = video[end]?.captureMicros ?? Infinity
      const [videoFrames, audioFrames] = await Promise.all([
        readFrames(selected, video.slice(index, end)),
        readFrames(selected, audio.slice(lowerBound(audio, from), lowerBound(audio, until)))
      ])
      if (current !== generation) return
      const continued = await review.play(
        videoFrames,
        audioFrames,
        { codec: videoTrack.codec },
        audioConfig,
        target,
        () => generation === current
      )
      if (!continued) return
      index = end
    }
    if (generation === current) status.value = 'Finished'
  } catch (error) {
    if (generation === current) status.value = error instanceof Error ? error.message : String(error)
  }
}

play.addEventListener('click', () => {
  void start()
})
seek.addEventListener('change', () => {
  void start()
})
