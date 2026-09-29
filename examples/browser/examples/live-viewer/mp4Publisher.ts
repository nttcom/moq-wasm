import { type IncomingSubscribeContext, MoqtClientWrapper } from '@moqt/moqtClient'
import init, {
  type MOQTClient,
  type Mp4AudioTrack,
  Mp4Index,
  type Mp4SampleTable,
  type Mp4VideoTrack
} from '../../pkg/moqt_client_wasm'
import { monotonicUnixMicros } from '../../utils/media/clock'
import { buildLocHeader, bytesToBase64 } from '../../utils/media/loc'
import { MEDIA_CATALOG_TRACK_NAME, type MsfTrack, buildMsfCatalogJson } from '../media/catalog'
import { getErrorMessage } from '../media/common'
import { StreamMonitor } from './streamMonitor'

const VIDEO_TRACK_NAME = 'video'
const AUDIO_TRACK_NAME = 'audio'
const SUBGROUP_ID = 0n
const PUBLISHER_PRIORITY = 0
/// draft-ietf-moq-transport-14 §10.4.1: 0x3 marks the end of the group.
const END_OF_GROUP_STATUS = 3
const MAX_REQUEST_ID = 1_000_000n
const MILLIS_PER_MICRO = 1 / 1_000
const ATOM_HEADER_LENGTH = 8
const LARGE_ATOM_HEADER_LENGTH = 16
const MOOV = 'moov'
const AUDIO_SAMPLE_KIND = 1
const MP3_CODEC = 'mp3'

type LogLevel = 'info' | 'warn' | 'error'

export type Mp4PublisherCallbacks = {
  onStatus(text: string): void
  onLog(level: LogLevel, message: string): void
  onVideoStarted(codec: string, reorderDelayMicros: number): void
  onVideoSample(annexB: Uint8Array, keyframe: boolean, captureMicros: number): void
  onStopped(): void
}

export type Mp4PublishOptions = {
  file: File
  url: string
  namespace: string[]
  authInfo: string
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
  reorderDelayMicros: number
  durationMicros: number
}

/// `sentStreams` records the subgroup streams sent to each subscriber the
/// way the viewer's Subscribe Streams card records the ones it receives.
export class Mp4Publisher {
  readonly sentStreams = new StreamMonitor()
  private readonly session = new MoqtClientWrapper()
  private running: Promise<void> | undefined
  private active = false
  private stopRequested = false
  private nextCatalogGroupId = 0n

  constructor(private readonly callbacks: Mp4PublisherCallbacks) {}

  async start(options: Mp4PublishOptions): Promise<void> {
    await this.stop()
    const media = await openMp4(options.file)
    try {
      const catalog = buildCatalogJson(options.namespace, media)
      this.nextCatalogGroupId = BigInt(monotonicUnixMicros())
      await this.session.connect(options.url, { maxRequestId: MAX_REQUEST_ID })
      this.session.setOnIncomingSubscribeHandler((context) => this.answerSubscribe(context, options.namespace, catalog))
      this.session.setOnConnectionClosedHandler(() => {
        this.callbacks.onLog('warn', 'publisher connection closed')
        this.stopRequested = true
      })
      await this.session.publishNamespace(options.namespace, options.authInfo)
    } catch (error) {
      media.index.free()
      throw error
    }
    this.stopRequested = false
    this.sentStreams.reset()
    this.active = true
    this.callbacks.onVideoStarted(media.video.codec, media.reorderDelayMicros)
    this.running = this.publish(media, options)
    this.callbacks.onStatus(
      `Publishing ${options.file.name} (${describeMedia(media)}) to ${options.namespace.join('/')}`
    )
    this.callbacks.onLog('info', `publishing ${options.file.name} to ${options.url} ${options.namespace.join('/')}`)
  }

  async stop(): Promise<void> {
    this.stopRequested = true
    await this.running
    this.running = undefined
  }

  get publishing(): boolean {
    return this.active
  }

  private async publish(media: Mp4Media, options: Mp4PublishOptions): Promise<void> {
    try {
      const completed = await this.pace(media, options)
      this.callbacks.onStatus(completed ? 'Publish finished' : 'Publish stopped')
    } catch (error) {
      this.callbacks.onStatus(`Publish failed: ${getErrorMessage(error)}`)
      this.callbacks.onLog('error', `publish: ${getErrorMessage(error)}`)
    } finally {
      this.active = false
      this.callbacks.onStopped()
      media.index.free()
      if (this.session.getConnectionStatus()) {
        await this.session.disconnect()
      }
    }
  }

  /// Samples go out in decode order, each at its decode time plus the file's
  /// reorder delay on the wall clock, the way a live encoder with B-frames
  /// emits them, and are stamped with their presentation time as the LOC
  /// capture timestamp: the viewer paces playback and resolves seek positions
  /// from it. Sending at the presentation time instead would hold a P-frame
  /// until it is shown and deliver the B-frames that precede it late.
  private async pace(media: Mp4Media, options: Mp4PublishOptions): Promise<boolean> {
    const client = this.requireClient()
    const video = new LocTrackSender(client, options.namespace, VIDEO_TRACK_NAME, false, this.sentStreams)
    const audio = media.audio && new LocTrackSender(client, options.namespace, AUDIO_TRACK_NAME, true, this.sentStreams)
    let nextGroupId = BigInt(monotonicUnixMicros())
    let groupId: bigint | undefined
    let passOriginMicros = monotonicUnixMicros()
    try {
      do {
        for (let index = 0; index < media.samples.size.length; index++) {
          if (this.stopRequested) {
            return false
          }
          const isVideo = media.samples.kind[index] !== AUDIO_SAMPLE_KIND
          const isSync = media.samples.sync[index] === 1
          const presentationMicros = passOriginMicros + (media.samples.ptsMicros[index] - media.firstPresentationMicros)
          const sendMicros =
            passOriginMicros +
            (media.samples.dtsMicros[index] - media.firstPresentationMicros) +
            media.reorderDelayMicros
          const data = await readSample(media.file, media.samples.offset[index], media.samples.size[index])
          await sleepUntilUnixMicros(sendMicros)
          if (isVideo && isSync) {
            groupId = nextGroupId
            nextGroupId += 1n
          }
          if (groupId === undefined) {
            continue
          }
          if (isVideo) {
            const annexB = media.index.annexBVideoSample(data, isSync)
            this.callbacks.onVideoSample(annexB, isSync, presentationMicros)
            await video.send(groupId, annexB, presentationMicros)
          } else if (audio) {
            await audio.send(groupId, data, presentationMicros)
          }
        }
        passOriginMicros += media.durationMicros
      } while (options.loop)
      return true
    } finally {
      if (this.session.getConnectionStatus()) {
        await video.closeGroup()
        await audio?.closeGroup()
      }
    }
  }

  private async answerSubscribe(
    context: IncomingSubscribeContext,
    namespace: string[],
    catalog: string
  ): Promise<void> {
    const { subscribe, isSuccess, code, respondOk, respondError } = context
    const trackName = subscribe.trackName ?? ''
    const requestedNamespace = (subscribe.trackNamespace ?? []).join('/')
    if (!isSuccess) {
      await respondError(BigInt(code), `subscribe error: code=${code}`)
      return
    }
    if (requestedNamespace !== namespace.join('/')) {
      await respondError(404n, 'unknown namespace')
      return
    }
    if (trackName === MEDIA_CATALOG_TRACK_NAME) {
      const trackAlias = await respondOk(0n)
      await this.sendCatalog(trackAlias, catalog)
      this.callbacks.onLog('info', `served ${MEDIA_CATALOG_TRACK_NAME} to a subscriber`)
      return
    }
    if (trackName === VIDEO_TRACK_NAME || trackName === AUDIO_TRACK_NAME) {
      await respondOk(0n)
      this.callbacks.onLog('info', `subscriber joined ${trackName}`)
      return
    }
    await respondError(404n, 'unknown track')
  }

  /// The relay keeps a track's cache across publisher sessions and treats a
  /// location it has already seen as a malformed track, so every catalog
  /// goes out in a fresh wall-clock seeded group like the media groups.
  private async sendCatalog(trackAlias: bigint, catalog: string): Promise<void> {
    const client = this.requireClient()
    const groupId = this.nextCatalogGroupId
    this.nextCatalogGroupId += 1n
    const payload = new TextEncoder().encode(catalog)
    this.sentStreams.label(trackAlias, MEDIA_CATALOG_TRACK_NAME)
    await client.sendSubgroupHeader(trackAlias, groupId, SUBGROUP_ID, PUBLISHER_PRIORITY)
    this.sentStreams.opened(trackAlias, groupId)
    await client.sendSubgroupObject(trackAlias, groupId, SUBGROUP_ID, 0n, undefined, payload, undefined)
    this.sentStreams.object(trackAlias, groupId, 0n, payload.byteLength, false)
    await client.sendSubgroupObject(
      trackAlias,
      groupId,
      SUBGROUP_ID,
      1n,
      END_OF_GROUP_STATUS,
      new Uint8Array(0),
      undefined
    )
    this.sentStreams.object(trackAlias, groupId, 1n, 0, true)
  }

  private requireClient(): MOQTClient {
    const client = this.session.getRawClient()
    if (!client) {
      throw new Error('publisher session is not connected')
    }
    return client
  }
}

/// One subgroup per group on every subscriber of the track. A subscriber
/// that arrives while a group is open only gets a subgroup from the next
/// group unless the track can be joined mid-group, which holds for audio but
/// not for video, whose groups start with the keyframe the rest depends on.
class LocTrackSender {
  private groupId: bigint | undefined
  private nextObjectId = 0n
  private readonly openSubgroups = new Map<string, bigint>()

  constructor(
    private readonly client: MOQTClient,
    private readonly namespace: string[],
    private readonly name: string,
    private readonly joinsMidGroup: boolean,
    private readonly sentStreams: StreamMonitor
  ) {}

  async send(groupId: bigint, payload: Uint8Array, captureMicros: number): Promise<void> {
    if (groupId !== this.groupId) {
      await this.closeGroup()
      this.groupId = groupId
      this.nextObjectId = 0n
    }
    const startsGroup = this.nextObjectId === 0n
    const locHeader = buildLocHeader({ captureTimestampMicros: captureMicros })
    for (const trackAlias of this.subscribers()) {
      const key = trackAlias.toString()
      if (this.openSubgroups.get(key) !== groupId) {
        if (!startsGroup && !this.joinsMidGroup) {
          continue
        }
        this.sentStreams.label(trackAlias, this.name)
        await this.client.sendSubgroupHeader(trackAlias, groupId, SUBGROUP_ID, PUBLISHER_PRIORITY)
        this.sentStreams.opened(trackAlias, groupId)
        this.openSubgroups.set(key, groupId)
      }
      await this.client.sendSubgroupObject(
        trackAlias,
        groupId,
        SUBGROUP_ID,
        this.nextObjectId,
        undefined,
        payload,
        locHeader
      )
      this.sentStreams.object(
        trackAlias,
        groupId,
        this.nextObjectId,
        payload.byteLength,
        false,
        Date.now(),
        captureMicros
      )
    }
    this.nextObjectId += 1n
  }

  async closeGroup(): Promise<void> {
    const groupId = this.groupId
    if (groupId === undefined) {
      return
    }
    const subscribed = new Set(this.subscribers().map((trackAlias) => trackAlias.toString()))
    for (const [key, openGroupId] of this.openSubgroups) {
      if (openGroupId === groupId && subscribed.has(key)) {
        await this.client.sendSubgroupObject(
          BigInt(key),
          groupId,
          SUBGROUP_ID,
          this.nextObjectId,
          END_OF_GROUP_STATUS,
          new Uint8Array(0),
          undefined
        )
        this.sentStreams.object(BigInt(key), groupId, this.nextObjectId, 0, true)
      }
    }
    this.openSubgroups.clear()
  }

  private subscribers(): bigint[] {
    return Array.from(this.client.getTrackSubscribers(this.namespace, this.name))
  }
}

async function sleepUntilUnixMicros(dueMicros: number): Promise<void> {
  const delayMs = (dueMicros - monotonicUnixMicros()) * MILLIS_PER_MICRO
  if (delayMs > 0) {
    await new Promise((resolve) => setTimeout(resolve, delayMs))
  }
}

async function openMp4(file: File): Promise<Mp4Media> {
  await init()
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

async function readSample(file: File, offset: number, size: number): Promise<Uint8Array> {
  return new Uint8Array(await file.slice(offset, offset + size).arrayBuffer())
}

function buildCatalogJson(namespace: string[], media: Mp4Media): string {
  const namespacePath = namespace.join('/')
  const tracks: MsfTrack[] = [
    {
      namespace: namespacePath,
      name: VIDEO_TRACK_NAME,
      packaging: 'loc',
      role: 'video',
      isLive: true,
      label: `${media.video.height}p`,
      codec: media.video.codec,
      mimeType: 'video/h264',
      width: media.video.width,
      height: media.video.height
    }
  ]
  if (media.audio) {
    tracks.push({
      namespace: namespacePath,
      name: AUDIO_TRACK_NAME,
      packaging: 'loc',
      role: 'audio',
      isLive: true,
      label: 'Audio',
      codec: media.audio.codec,
      mimeType: media.audio.codec === MP3_CODEC ? 'audio/mpeg' : 'audio/aac',
      samplerate: media.audio.sampleRate,
      channelConfig: channelConfigLabel(media.audio.channels),
      initData: media.audio.audioSpecificConfig && bytesToBase64(media.audio.audioSpecificConfig)
    })
  }
  return buildMsfCatalogJson(tracks)
}

function channelConfigLabel(channels: number): string {
  switch (channels) {
    case 1:
      return 'mono'
    case 2:
      return 'stereo'
    default:
      return `${channels}ch`
  }
}

function describeMedia(media: Mp4Media): string {
  const video = `${media.video.width}x${media.video.height} ${media.video.codec}`
  return media.audio ? `${video}, ${media.audio.codec}` : video
}
