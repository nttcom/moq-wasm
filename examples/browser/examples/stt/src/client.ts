import { MoqtClientWrapper } from '@moqt/moqtClient'
import type { MOQTClient } from '../../../pkg/moqt_client_wasm'
import { sendAudioChunkViaMoqt } from '../../../utils/media/audioTransport'
import { MediaTransportState } from '../../../utils/media/transportState'
import type { PipelineObject } from './types'

const AUTH_INFO = 'secret'
const CATALOG_TRACK_NAME = 'catalog'
const AUDIO_TRACK_NAME = 'audio'
const PIPELINE_TRACK_NAME = 'pipeline'
const REPLY_TRACK_NAME = 'reply'
const AUDIO_SAMPLE_RATE = 48_000
const AUDIO_BITRATE = 64_000
const AUDIO_CHUNKS_PER_GROUP = 50
const FILTER_TYPE_NEXT_GROUP_START = 1
const OBJECT_STATUS_END_OF_GROUP = 3

type AudioEncoderWorkerMessage =
  | {
      type: 'chunk'
      chunk: EncodedAudioChunk
      metadata: EncodedAudioChunkMetadata | undefined
      captureTimestampMicros?: number
    }
  | { type: 'bitrate'; media: 'audio'; kbps: number }
  | { type: 'configError'; media: 'audio'; reason: string }

export type ClientCallbacks = {
  onPipelineObject: (object: PipelineObject) => void
  onReplyPacket: (turn: number, packet: Uint8Array) => void
  onAudioObjectsSent: (count: number) => void
  onStatus: (status: string) => void
}

/** Publishes the microphone as an Opus MoQT track and subscribes to the
 * server's pipeline and reply tracks. */
export class VoicePipelineClient {
  private readonly moqt = new MoqtClientWrapper()
  private readonly encoder = new Worker(new URL('../../../utils/media/encoders/audioEncoder.ts', import.meta.url), {
    type: 'module'
  })
  private readonly sentAt = new Map<string, number>()
  private transportState = new MediaTransportState()
  private namespace: string[] = []
  private stream: MediaStream | null = null
  private audioObjectsSent = 0

  constructor(private readonly callbacks: ClientCallbacks) {}

  /** Browser clock (`performance.now()`) when the audio object was sent. */
  sentAtOf(groupId: number, objectId: number): number | undefined {
    return this.sentAt.get(`${groupId}:${objectId}`)
  }

  async start(url: string, namespace: string): Promise<void> {
    this.namespace = namespace
      .split('/')
      .map((part) => part.trim())
      .filter((part) => part.length > 0)

    this.callbacks.onStatus('requesting microphone')
    this.stream = await navigator.mediaDevices.getUserMedia({ audio: true })

    this.callbacks.onStatus(`connecting to ${url}`)
    await this.moqt.connect(url)
    this.answerSubscribes()
    await this.subscribeResultTrack(PIPELINE_TRACK_NAME, (payload) =>
      this.callbacks.onPipelineObject(JSON.parse(new TextDecoder().decode(payload)))
    )
    await this.subscribeResultTrack(REPLY_TRACK_NAME, (payload, groupId) =>
      this.callbacks.onReplyPacket(groupId, payload)
    )
    await this.moqt.publishNamespace(this.namespace, AUTH_INFO)
    this.publishMicrophone(this.stream)
    this.callbacks.onStatus(`publishing ${namespace}/${AUDIO_TRACK_NAME}`)
  }

  async stop(): Promise<void> {
    this.encoder.onmessage = null
    this.stream?.getTracks().forEach((track) => track.stop())
    this.stream = null
    await this.moqt.disconnect()
    this.callbacks.onStatus('disconnected')
  }

  private requireClient(): MOQTClient {
    const client = this.moqt.getRawClient()
    if (!client) {
      throw new Error('MoQT client is not connected')
    }
    return client
  }

  private async subscribeResultTrack(
    trackName: string,
    onPayload: (payload: Uint8Array, groupId: number) => void
  ): Promise<void> {
    const { subscribeOk } = await this.moqt.subscribe(this.namespace, trackName, AUTH_INFO, {
      filterType: FILTER_TYPE_NEXT_GROUP_START,
      forward: true
    })
    this.moqt.setOnSubgroupObjectHandler(subscribeOk.trackAlias, (groupId, object) => {
      if (object.objectPayloadLength > 0) {
        onPayload(new Uint8Array(object.objectPayload), Number(groupId))
      }
    })
  }

  private answerSubscribes(): void {
    this.moqt.setOnIncomingSubscribeHandler(async ({ subscribe, isSuccess, code, respondOk, respondError }) => {
      if (!isSuccess) {
        await respondError(BigInt(code), `subscribe error: code=${code}`)
        return
      }
      if ((subscribe.trackNamespace ?? []).join('/') !== this.namespace.join('/')) {
        await respondError(404n, 'unknown namespace')
        return
      }
      switch (subscribe.trackName) {
        case CATALOG_TRACK_NAME: {
          const trackAlias = await respondOk(0n)
          await this.sendCatalog(trackAlias)
          return
        }
        case AUDIO_TRACK_NAME:
          await respondOk(0n)
          return
        default:
          await respondError(404n, 'unknown track')
      }
    })
  }

  private async sendCatalog(trackAlias: bigint): Promise<void> {
    const catalog = {
      version: 1,
      isComplete: true,
      tracks: [
        {
          namespace: this.namespace.join('/'),
          name: AUDIO_TRACK_NAME,
          packaging: 'loc',
          role: 'audio',
          isLive: true,
          codec: 'opus',
          bitrate: AUDIO_BITRATE,
          samplerate: AUDIO_SAMPLE_RATE,
          channelConfig: 'mono'
        }
      ]
    }
    const client = this.requireClient()
    const payload = new TextEncoder().encode(JSON.stringify(catalog))
    await client.sendSubgroupHeader(trackAlias, 0n, 0n, 0)
    await client.sendSubgroupObject(trackAlias, 0n, 0n, 0n, undefined, payload, undefined)
  }

  private publishMicrophone(stream: MediaStream): void {
    const client = this.requireClient()
    const [audioTrack] = stream.getAudioTracks()
    this.transportState = new MediaTransportState()
    this.audioObjectsSent = 0

    this.encoder.onmessage = async (event: MessageEvent<AudioEncoderWorkerMessage>) => {
      const data = event.data
      if (data.type === 'configError') {
        this.callbacks.onStatus(`audio encoder unsupported: ${data.reason}`)
        return
      }
      if (data.type !== 'chunk') {
        return
      }
      const trackAliases = Array.from(client.getTrackSubscribers(this.namespace, AUDIO_TRACK_NAME), (alias) =>
        BigInt(alias)
      )
      if (!trackAliases.length) {
        return
      }
      if (this.audioObjectsSent > 0 && this.audioObjectsSent % AUDIO_CHUNKS_PER_GROUP === 0) {
        await this.finishAudioGroup(client, trackAliases)
      }
      const groupId = this.transportState.getAudioGroupId()
      const objectId = this.transportState.getAudioObjectNumber()
      await sendAudioChunkViaMoqt({
        chunk: data.chunk,
        metadata: data.metadata,
        captureTimestampMicros: data.captureTimestampMicros,
        trackAliases,
        client,
        transportState: this.transportState
      })
      this.sentAt.set(`${groupId}:${objectId}`, performance.now())
      this.audioObjectsSent += 1
      this.callbacks.onAudioObjectsSent(this.audioObjectsSent)
    }

    this.encoder.postMessage({
      type: 'config',
      config: {
        codec: 'opus',
        sampleRate: AUDIO_SAMPLE_RATE,
        numberOfChannels: 1,
        bitrate: AUDIO_BITRATE
      }
    })
    const processor = new MediaStreamTrackProcessor({ track: audioTrack })
    const audioStream = processor.readable
    this.encoder.postMessage({ type: 'audioStream', audioStream }, [audioStream])
  }

  /** The wasm client closes a subgroup stream when it sends EndOfGroup, and
   * sequential readers only move on once the stream has ended. */
  private async finishAudioGroup(client: MOQTClient, trackAliases: bigint[]): Promise<void> {
    const groupId = this.transportState.getAudioGroupId()
    const endObjectNumber = this.transportState.getAudioObjectNumber()
    for (const alias of trackAliases) {
      await client.sendSubgroupObject(
        alias,
        groupId,
        0n,
        endObjectNumber,
        OBJECT_STATUS_END_OF_GROUP,
        new Uint8Array(0),
        undefined
      )
    }
    this.transportState.advanceAudioGroup()
  }
}
