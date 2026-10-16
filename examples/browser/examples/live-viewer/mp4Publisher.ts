import { Mp4Broadcast, Mp4Index, type Mp4SampleTable, type Mp4VideoTrack, type Mp4AudioTrack } from '../../pkg/moqt'
import { loadMoqt } from '../../lib/moqt/loadMoqt'
import { monotonicUnixMicros } from '../../utils/media/clock'
import type { PublishPreview } from './publishPreview'
import { StreamMonitor, type StreamRecord } from './streamMonitor'
import { type StatusState, getErrorMessage } from '../../utils/page'

const MILLIS_PER_MICRO = 1 / 1_000
const ATOM_HEADER_LENGTH = 8
const LARGE_ATOM_HEADER_LENGTH = 16
const MOOV = 'moov'
const AUDIO_SAMPLE_KIND = 1

type LogLevel = 'info' | 'warn' | 'error'

export type Mp4PublisherCallbacks = {
  onStatus(text: string, state: StatusState): void
  onLog(level: LogLevel, message: string): void
}

export type Mp4PublishOptions = {
  file: File
  url: string
  namespace: string[]
  loop: boolean
}

/// The columns of `Mp4SampleTable` are copied out of wasm memory on every
/// access, so they are read once.
type SampleColumns = {
  kind: Uint8Array
  offset: Float64Array
  size: Uint32Array
  dtsMicros: Float64Array
  ptsMicros: Float64Array
  sync: Uint8Array
}

type Mp4Media = {
  file: File
  index: Mp4Index
  samples: SampleColumns
  video: Mp4VideoTrack
  audio?: Mp4AudioTrack
  firstPresentationMicros: number
  /// Timestamps pushed to the broadcast count from this, so neither
  /// presentation nor decode time is negative.
  firstDecodeMicros: number
  reorderDelayMicros: number
  durationMicros: number
}

/// Reads the samples of an MP4 file and paces them like a live encoder into
/// `Mp4Broadcast`, which publishes them as live-ingest does.
export class Mp4Publisher {
  readonly sentStreams = new StreamMonitor()
  private broadcast: Mp4Broadcast | undefined
  private readonly trackAliases = new Map<string, bigint>()
  private running: Promise<void> | undefined
  private stopRequested = false

  constructor(
    private readonly callbacks: Mp4PublisherCallbacks,
    private readonly preview: PublishPreview
  ) {}

  async start(options: Mp4PublishOptions): Promise<void> {
    await this.stop()
    const media = await openMp4(options.file)
    let broadcast: Mp4Broadcast
    try {
      this.sentStreams.reset()
      this.trackAliases.clear()
      broadcast = new Mp4Broadcast(options.url, options.namespace, media.index)
      try {
        await broadcast.announce()
      } catch (error) {
        broadcast.free()
        throw error
      }
    } catch (error) {
      media.index.free()
      throw error
    }
    this.broadcast = broadcast
    this.stopRequested = false
    this.preview.start(media.video.codec, media.reorderDelayMicros)
    this.running = this.publish(broadcast, media, options)
    this.callbacks.onStatus(
      `Publishing ${options.file.name} (${describeMedia(media)}) to ${options.namespace.join('/')}`,
      'ok'
    )
    this.callbacks.onLog('info', `publishing ${options.file.name} to ${options.url} ${options.namespace.join('/')}`)
  }

  async stop(): Promise<void> {
    this.stopRequested = true
    await this.running
    this.running = undefined
  }

  get publishing(): boolean {
    return this.broadcast !== undefined
  }

  /// Refreshes `sentStreams` from the broadcast and returns its snapshot.
  sentStreamRecords(): StreamRecord[] {
    if (this.broadcast) {
      this.sentStreams.replace(this.broadcast.streams().map((stream) => this.toStreamRecord(stream)))
    }
    return this.sentStreams.snapshot()
  }

  /// The final status is reported once the session is closed, so a reader of
  /// `Publish stopped` knows the relay has seen the publisher leave.
  private async publish(broadcast: Mp4Broadcast, media: Mp4Media, options: Mp4PublishOptions): Promise<void> {
    let status: Parameters<Mp4PublisherCallbacks['onStatus']>
    try {
      const completed = await this.pace(broadcast, media, options)
      status = [completed ? 'Publish finished' : 'Publish stopped', 'idle']
    } catch (error) {
      status = [`Publish failed: ${getErrorMessage(error)}`, 'error']
      this.callbacks.onLog('error', `publish: ${getErrorMessage(error)}`)
    }
    this.broadcast = undefined
    this.preview.stop()
    media.index.free()
    await broadcast.close()
    broadcast.free()
    this.callbacks.onStatus(...status)
  }

  /// Samples go out in decode order, each at its decode time plus the file's
  /// reorder delay on the wall clock, the way a live encoder with B-frames
  /// emits them. Sending at the presentation time instead would hold a
  /// P-frame until it is shown and deliver the B-frames that precede it late.
  private async pace(broadcast: Mp4Broadcast, media: Mp4Media, options: Mp4PublishOptions): Promise<boolean> {
    let passOriginMicros = monotonicUnixMicros()
    let passOffsetMicros = 0
    do {
      for (let index = 0; index < media.samples.size.length; index++) {
        if (this.stopRequested) {
          return false
        }
        const sendMicros =
          passOriginMicros + (media.samples.dtsMicros[index] - media.firstPresentationMicros) + media.reorderDelayMicros
        const payload = await readSamplePayload(media, index)
        await sleepUntilUnixMicros(sendMicros)
        const ptsMicros = passOffsetMicros + media.samples.ptsMicros[index] - media.firstDecodeMicros
        if (isAudioSample(media, index)) {
          broadcast.pushAudio(payload, ptsMicros)
          continue
        }
        const isSync = media.samples.sync[index] === 1
        const dtsMicros = passOffsetMicros + media.samples.dtsMicros[index] - media.firstDecodeMicros
        this.preview.decode(
          payload,
          isSync,
          passOriginMicros + (media.samples.ptsMicros[index] - media.firstPresentationMicros)
        )
        broadcast.pushVideo(payload, isSync, ptsMicros, dtsMicros)
      }
      passOriginMicros += media.durationMicros
      passOffsetMicros += media.durationMicros
    } while (options.loop)
    return true
  }

  private toStreamRecord(stream: ReturnType<Mp4Broadcast['streams']>[number]): StreamRecord {
    try {
      let trackAlias = this.trackAliases.get(stream.track)
      if (trackAlias === undefined) {
        trackAlias = BigInt(this.trackAliases.size)
        this.trackAliases.set(stream.track, trackAlias)
      }
      return {
        kind: 'subscribe',
        trackAlias,
        track: stream.track,
        groupId: stream.groupId,
        lastGroupId: stream.groupId,
        openedAt: stream.openedAtMs,
        finishedAt: stream.finishedAtMs,
        bytes: stream.bytes,
        objects: stream.objectSentAtMs.length,
        receivedAtByObjectId: new Map(
          Array.from(stream.objectSentAtMs, (sentAt, objectId) => [BigInt(objectId), sentAt])
        ),
        captureAtByObjectId: new Map(),
        receivedAtByGroup: new Map(),
        captureAtByGroup: new Map()
      }
    } finally {
      stream.free()
    }
  }
}

function isAudioSample(media: Mp4Media, index: number): boolean {
  return media.samples.kind[index] === AUDIO_SAMPLE_KIND
}

async function readSamplePayload(media: Mp4Media, index: number): Promise<Uint8Array> {
  const offset = media.samples.offset[index]
  const data = new Uint8Array(await media.file.slice(offset, offset + media.samples.size[index]).arrayBuffer())
  return isAudioSample(media, index) ? data : media.index.annexBVideoSample(data, media.samples.sync[index] === 1)
}

async function sleepUntilUnixMicros(dueMicros: number): Promise<void> {
  const delayMs = (dueMicros - monotonicUnixMicros()) * MILLIS_PER_MICRO
  if (delayMs > 0) {
    await new Promise((resolve) => setTimeout(resolve, delayMs))
  }
}

async function openMp4(file: File): Promise<Mp4Media> {
  await loadMoqt()
  const index = new Mp4Index(await readMoovAtom(file))
  try {
    const video = index.video()
    if (!video) {
      throw new Error('the file has no H.264 video track')
    }
    const samples = readSampleColumns(index.samples())
    return {
      file,
      index,
      samples,
      video,
      audio: index.audio(),
      firstPresentationMicros: samples.ptsMicros.reduce((earliest, pts) => Math.min(earliest, pts), Infinity),
      firstDecodeMicros: samples.dtsMicros.reduce((earliest, dts) => Math.min(earliest, dts), Infinity),
      reorderDelayMicros: index.reorderDelayMicros(),
      durationMicros: index.durationMicros()
    }
  } catch (error) {
    index.free()
    throw error
  }
}

/// ISO/IEC 14496-12 §4.2: a top-level atom starts with its 32-bit size and
/// type; a size of 1 puts the 64-bit size next and a size of 0 runs to the
/// end of the file. Only the headers are read until the `moov` atom is found.
async function readMoovAtom(file: File): Promise<Uint8Array> {
  let offset = 0
  while (offset + ATOM_HEADER_LENGTH <= file.size) {
    const header = new DataView(await file.slice(offset, offset + LARGE_ATOM_HEADER_LENGTH).arrayBuffer())
    const kind = String.fromCharCode(header.getUint8(4), header.getUint8(5), header.getUint8(6), header.getUint8(7))
    let size = header.getUint32(0)
    if (size === 1) {
      size = Number(header.getBigUint64(ATOM_HEADER_LENGTH))
    } else if (size === 0) {
      size = file.size - offset
    }
    if (size < ATOM_HEADER_LENGTH) {
      throw new Error(`invalid ${kind} atom size ${size}`)
    }
    if (kind === MOOV) {
      return new Uint8Array(await file.slice(offset, offset + size).arrayBuffer())
    }
    offset += size
  }
  throw new Error('the file has no moov atom')
}

function readSampleColumns(table: Mp4SampleTable): SampleColumns {
  try {
    return {
      kind: table.kind,
      offset: table.offset,
      size: table.size,
      dtsMicros: table.dtsMicros,
      ptsMicros: table.ptsMicros,
      sync: table.sync
    }
  } finally {
    table.free()
  }
}

function describeMedia(media: Mp4Media): string {
  const video = `${media.video.width}x${media.video.height} ${media.video.codec}`
  return media.audio ? `${video}, ${media.audio.codec}` : video
}
