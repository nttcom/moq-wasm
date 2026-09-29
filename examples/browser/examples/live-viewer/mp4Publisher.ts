import {
  type IncomingFetchContext,
  type IncomingSubscribeContext,
  type IncomingTrackStatusContext,
  MoqtClientWrapper,
  RequestErrorCode
} from '@moqt/moqtClient'
import init, {
  type MOQTClient,
  type Mp4AudioTrack,
  Mp4Index,
  type Mp4SampleTable,
  type Mp4VideoTrack
} from '../../pkg/moqt_client_wasm'
import { monotonicUnixMicros } from '../../utils/media/clock'
import { buildLocHeader, bytesToBase64 } from '../../utils/media/loc'
import { OBJECT_STATUS_END_OF_GROUP } from '../../utils/media/objectStatus'
import { MEDIA_CATALOG_TRACK_NAME, type MsfTrack, buildMsfCatalogJson } from '../media/catalog'
import { type StatusState, getErrorMessage } from '../media/common'
import { PublishedGroupLog, type ReplayObject, type ReplayTrack, answerFetch } from './fetchReplay'
import type { PublishPreview } from './publishPreview'
import { StreamMonitor } from './streamMonitor'

const VIDEO_TRACK_NAME = 'video'
const AUDIO_TRACK_NAME = 'audio'
const TIMELINE_TRACK_NAME = 'timeline'
const SUBGROUP_ID = 0n
const PUBLISHER_PRIORITY = 0
const MAX_REQUEST_ID = 1_000_000n
const MILLIS_PER_MICRO = 1 / 1_000
/// The relay's default cache TTL: an older record names a group that can no longer be fetched.
const TIMELINE_RETENTION_MS = 60_000
const ATOM_HEADER_LENGTH = 8
const LARGE_ATOM_HEADER_LENGTH = 16
const MOOV = 'moov'
const AUDIO_SAMPLE_KIND = 1

type LogLevel = 'info' | 'warn' | 'error'

/// draft-ietf-moq-msf-01 §7.1.1: [presentation time ms, [group id, object id], encode wallclock ms].
type MediaTimelineRecord = [number, [number, number], number]

export type Mp4PublisherCallbacks = {
  onStatus(text: string, state: StatusState): void
  onLog(level: LogLevel, message: string): void
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

export class Mp4Publisher {
  readonly sentStreams = new StreamMonitor()
  private readonly session = new MoqtClientWrapper()
  private running: Promise<void> | undefined
  private active = false
  private stopRequested = false
  private nextCatalogGroupId = 0n
  private catalogGroups = new Map<bigint, Uint8Array>()
  private timelineGroups = new Map<bigint, Uint8Array>()
  private mediaGroups = new PublishedGroupLog(0)

  constructor(
    private readonly callbacks: Mp4PublisherCallbacks,
    private readonly preview: PublishPreview
  ) {}

  async start(options: Mp4PublishOptions): Promise<void> {
    await this.stop()
    const media = await openMp4(options.file)
    try {
      const catalog = buildCatalogJson(options.namespace, media)
      this.nextCatalogGroupId = BigInt(monotonicUnixMicros())
      this.catalogGroups = new Map()
      this.timelineGroups = new Map()
      this.mediaGroups = new PublishedGroupLog(media.samples.size.length)
      await this.session.connect(options.url, { maxRequestId: MAX_REQUEST_ID })
      this.session.setOnIncomingSubscribeHandler((context) => this.answerSubscribe(context, options.namespace, catalog))
      const replayTracks = this.replayTracks(media)
      this.session.setOnIncomingTrackStatusHandler((context) =>
        this.answerTrackStatus(context, options.namespace, replayTracks)
      )
      this.session.setOnIncomingFetchHandler((context) => this.answerFetch(context, options.namespace, replayTracks))
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
    this.preview.start(media.video.codec, media.reorderDelayMicros)
    this.running = this.publish(media, options)
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
    return this.active
  }

  private async publish(media: Mp4Media, options: Mp4PublishOptions): Promise<void> {
    try {
      const completed = await this.pace(media, options)
      this.callbacks.onStatus(completed ? 'Publish finished' : 'Publish stopped', 'idle')
    } catch (error) {
      this.callbacks.onStatus(`Publish failed: ${getErrorMessage(error)}`, 'error')
      this.callbacks.onLog('error', `publish: ${getErrorMessage(error)}`)
    } finally {
      this.active = false
      this.preview.stop()
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
    const video = new TrackSender(client, options.namespace, VIDEO_TRACK_NAME, false, this.sentStreams)
    const audio = media.audio && new TrackSender(client, options.namespace, AUDIO_TRACK_NAME, true, this.sentStreams)
    const timeline = new TrackSender(client, options.namespace, TIMELINE_TRACK_NAME, false, this.sentStreams)
    const records: MediaTimelineRecord[] = []
    let nextTimelineGroupId = BigInt(monotonicUnixMicros())
    let nextGroupId = BigInt(monotonicUnixMicros())
    let groupId: bigint | undefined
    const originMicros = monotonicUnixMicros()
    let passOriginMicros = originMicros
    try {
      do {
        for (let index = 0; index < media.samples.size.length; index++) {
          if (this.stopRequested) {
            return false
          }
          const isVideo = !isAudioSample(media, index)
          const isSync = media.samples.sync[index] === 1
          const presentationMicros = samplePresentationMicros(media, passOriginMicros, index)
          const sendMicros =
            passOriginMicros +
            (media.samples.dtsMicros[index] - media.firstPresentationMicros) +
            media.reorderDelayMicros
          const payload = await readSamplePayload(media, index)
          await sleepUntilUnixMicros(sendMicros)
          if (isVideo && isSync) {
            groupId = nextGroupId
            nextGroupId += 1n
            this.mediaGroups.startGroup(groupId, index, passOriginMicros)
          }
          if (groupId === undefined) {
            continue
          }
          if (isVideo) {
            this.preview.decode(payload, isSync, presentationMicros)
            await video.send(groupId, payload, presentationMicros)
            if (isSync) {
              records.push([
                Math.floor((presentationMicros - originMicros) * MILLIS_PER_MICRO),
                [Number(groupId), 0],
                Math.floor(presentationMicros * MILLIS_PER_MICRO)
              ])
              while (records[0][0] < records[records.length - 1][0] - TIMELINE_RETENTION_MS) {
                records.shift()
              }
              const document = new TextEncoder().encode(JSON.stringify(records))
              const timelineGroupId = nextTimelineGroupId++
              this.timelineGroups.set(timelineGroupId, document)
              for (const retainedGroupId of this.timelineGroups.keys()) {
                if (this.timelineGroups.size <= records.length) {
                  break
                }
                this.timelineGroups.delete(retainedGroupId)
              }
              await timeline.send(timelineGroupId, document)
            }
          } else if (audio) {
            await audio.send(groupId, payload, presentationMicros)
          }
        }
        passOriginMicros += media.durationMicros
        if (options.loop) {
          this.mediaGroups.startPass(passOriginMicros)
        }
      } while (options.loop)
      return true
    } finally {
      if (this.session.getConnectionStatus()) {
        await video.closeGroup()
        await audio?.closeGroup()
        await timeline.closeGroup()
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
    if (trackName === VIDEO_TRACK_NAME || trackName === AUDIO_TRACK_NAME || trackName === TIMELINE_TRACK_NAME) {
      await respondOk(0n)
      this.callbacks.onLog('info', `subscriber joined ${trackName}`)
      return
    }
    await respondError(404n, 'unknown track')
  }

  private async answerTrackStatus(
    context: IncomingTrackStatusContext,
    namespace: string[],
    replayTracks: Map<string, ReplayTrack>
  ): Promise<void> {
    const { trackStatus, respondOk, respondError } = context
    if (findReplayTrack(replayTracks, namespace, trackStatus.trackNamespace, trackStatus.trackName)) {
      await respondOk()
      return
    }
    await respondError(RequestErrorCode.TrackDoesNotExist, 'unknown track')
  }

  private async answerFetch(
    context: IncomingFetchContext,
    namespace: string[],
    replayTracks: Map<string, ReplayTrack>
  ): Promise<void> {
    const { fetch } = context
    const track = findReplayTrack(replayTracks, namespace, fetch.trackNamespace, fetch.trackName)
    try {
      if (!track) {
        await context.respondError(RequestErrorCode.TrackDoesNotExist, 'unknown track')
        return
      }
      await answerFetch(context, track)
      this.callbacks.onLog(
        'info',
        `answered FETCH ${fetch.trackName} ${fetch.startGroupId}:${fetch.startObjectId}-${fetch.endGroupId}:${fetch.endObjectId}`
      )
    } catch (error) {
      if (!context.cancelSignal.aborted) {
        this.callbacks.onLog('error', `FETCH ${fetch.trackName}: ${getErrorMessage(error)}`)
      }
    }
  }

  private replayTracks(media: Mp4Media): Map<string, ReplayTrack> {
    const tracks = new Map<string, ReplayTrack>([
      [MEDIA_CATALOG_TRACK_NAME, documentReplayTrack(this.catalogGroups, () => true)],
      [VIDEO_TRACK_NAME, mediaReplayTrack(media, this.mediaGroups, false)],
      [
        TIMELINE_TRACK_NAME,
        documentReplayTrack(this.timelineGroups, (groupId) => groupId !== lastKey(this.timelineGroups))
      ]
    ])
    if (media.audio) {
      tracks.set(AUDIO_TRACK_NAME, mediaReplayTrack(media, this.mediaGroups, true))
    }
    return tracks
  }

  /// The relay keeps a track's cache across publisher sessions and treats a
  /// location it has already seen as a malformed track, so every catalog
  /// goes out in a fresh wall-clock seeded group like the media groups.
  private async sendCatalog(trackAlias: bigint, catalog: string): Promise<void> {
    const client = this.requireClient()
    const groupId = this.nextCatalogGroupId
    this.nextCatalogGroupId += 1n
    const payload = new TextEncoder().encode(catalog)
    this.catalogGroups.set(groupId, payload)
    this.sentStreams.label(trackAlias, MEDIA_CATALOG_TRACK_NAME)
    await client.sendSubgroupHeader(trackAlias, groupId, SUBGROUP_ID, PUBLISHER_PRIORITY)
    this.sentStreams.opened(trackAlias, groupId)
    await client.sendSubgroupObject(trackAlias, groupId, SUBGROUP_ID, 0n, undefined, payload, undefined)
    this.sentStreams.object(trackAlias, groupId, 0n, payload.byteLength, false)
    await sendEndOfGroup(client, this.sentStreams, trackAlias, groupId, 1n)
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
class TrackSender {
  private groupId: bigint | undefined
  private nextObjectId = 0n
  private readonly openSubgroups = new Map<bigint, bigint>()

  constructor(
    private readonly client: MOQTClient,
    private readonly namespace: string[],
    private readonly name: string,
    private readonly joinsMidGroup: boolean,
    private readonly sentStreams: StreamMonitor
  ) {}

  async send(groupId: bigint, payload: Uint8Array, captureMicros?: number): Promise<void> {
    if (groupId !== this.groupId) {
      await this.closeGroup()
      this.groupId = groupId
      this.nextObjectId = 0n
    }
    const startsGroup = this.nextObjectId === 0n
    const locHeader = captureMicros === undefined ? undefined : captureLocHeader(captureMicros)
    for (const trackAlias of this.subscribers()) {
      if (this.openSubgroups.get(trackAlias) !== groupId) {
        if (!startsGroup && !this.joinsMidGroup) {
          continue
        }
        this.sentStreams.label(trackAlias, this.name)
        await this.client.sendSubgroupHeader(trackAlias, groupId, SUBGROUP_ID, PUBLISHER_PRIORITY)
        this.sentStreams.opened(trackAlias, groupId)
        this.openSubgroups.set(trackAlias, groupId)
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
    const subscribed = new Set(this.subscribers())
    for (const [trackAlias, openGroupId] of this.openSubgroups) {
      if (openGroupId === groupId && subscribed.has(trackAlias)) {
        await sendEndOfGroup(this.client, this.sentStreams, trackAlias, groupId, this.nextObjectId)
      }
    }
    this.openSubgroups.clear()
  }

  private subscribers(): bigint[] {
    return Array.from(this.client.getTrackSubscribers(this.namespace, this.name))
  }
}

async function sendEndOfGroup(
  client: MOQTClient,
  sentStreams: StreamMonitor,
  trackAlias: bigint,
  groupId: bigint,
  objectId: bigint
): Promise<void> {
  await client.sendSubgroupObject(
    trackAlias,
    groupId,
    SUBGROUP_ID,
    objectId,
    OBJECT_STATUS_END_OF_GROUP,
    new Uint8Array(0),
    undefined
  )
  sentStreams.object(trackAlias, groupId, objectId, 0, true)
}

function findReplayTrack(
  replayTracks: Map<string, ReplayTrack>,
  namespace: string[],
  requestedNamespace: string[],
  trackName: string
): ReplayTrack | undefined {
  return requestedNamespace.join('/') === namespace.join('/') ? replayTracks.get(trackName) : undefined
}

/// A catalog or media timeline group holds one document; the group is
/// closed with End of Group once no later document can land in it.
function documentReplayTrack(documents: Map<bigint, Uint8Array>, isClosed: (groupId: bigint) => boolean): ReplayTrack {
  return {
    groupIds: () => Array.from(documents.keys()),
    async *objects(groupId: bigint) {
      const document = documents.get(groupId)
      if (!document) {
        return
      }
      yield replayObject(0n, document)
      if (isClosed(groupId)) {
        yield endOfGroupObject(1n)
      }
    }
  }
}

function lastKey<K, V>(map: Map<K, V>): K | undefined {
  let last: K | undefined
  for (const key of map.keys()) {
    last = key
  }
  return last
}

/// Replays a group exactly as `Mp4Publisher.pace` sent it: the relay keeps
/// the first copy of an object and treats different bytes at the same
/// location as a malformed track.
function mediaReplayTrack(media: Mp4Media, groups: PublishedGroupLog, audio: boolean): ReplayTrack {
  return {
    groupIds: () => groups.groupIds(),
    async *objects(groupId: bigint) {
      let objectId = 0n
      for (const span of groups.spans(groupId)) {
        for (let index = span.firstSample; index < span.endSample; index++) {
          if (isAudioSample(media, index) !== audio) {
            continue
          }
          const captureMicros = samplePresentationMicros(media, span.passOriginMicros, index)
          yield replayObject(objectId, await readSamplePayload(media, index), captureLocHeader(captureMicros))
          objectId += 1n
        }
      }
      if (groups.isClosed(groupId)) {
        yield endOfGroupObject(objectId)
      }
    }
  }
}

function replayObject(objectId: bigint, payload: Uint8Array, locHeader?: unknown): ReplayObject {
  return { objectId, subgroupId: SUBGROUP_ID, publisherPriority: PUBLISHER_PRIORITY, payload, locHeader }
}

function endOfGroupObject(objectId: bigint): ReplayObject {
  return { ...replayObject(objectId, new Uint8Array(0)), objectStatus: OBJECT_STATUS_END_OF_GROUP }
}

function isAudioSample(media: Mp4Media, index: number): boolean {
  return media.samples.kind[index] === AUDIO_SAMPLE_KIND
}

function samplePresentationMicros(media: Mp4Media, passOriginMicros: number, index: number): number {
  return passOriginMicros + (media.samples.ptsMicros[index] - media.firstPresentationMicros)
}

function captureLocHeader(captureMicros: number) {
  return buildLocHeader({ captureTimestampMicros: captureMicros })
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
      width: media.video.width,
      height: media.video.height
    },
    {
      namespace: namespacePath,
      name: TIMELINE_TRACK_NAME,
      packaging: 'mediatimeline',
      role: 'mediatimeline',
      isLive: true,
      label: 'Media timeline',
      mimeType: 'application/json',
      depends: [VIDEO_TRACK_NAME]
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
      samplerate: media.audio.sampleRate,
      channelConfig: channelConfigLabel(media.audio.channels),
      initData: media.audio.audioSpecificConfig && bytesToBase64(media.audio.audioSpecificConfig)
    })
  }
  return buildMsfCatalogJson(tracks)
}

function channelConfigLabel(channels: number): string {
  return channels === 1 ? 'mono' : channels === 2 ? 'stereo' : `${channels}ch`
}

function describeMedia(media: Mp4Media): string {
  const video = `${media.video.width}x${media.video.height} ${media.video.codec}`
  return media.audio ? `${video}, ${media.audio.codec}` : video
}
