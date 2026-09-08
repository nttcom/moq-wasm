import { MoqtClientWrapper } from '@moqt/moqtClient'
import type { MOQTClient } from '../../pkg/moqt_client_wasm'
import { sendAudioChunkViaMoqt } from '../../utils/media/audioTransport'
import { MediaTransportState } from '../../utils/media/transportState'
import { configureRelayUrlControls } from '../../utils/relayPresets'
import { getErrorMessage, parseTrackNamespace, setStatusText } from '../media/common'

const AUTH_INFO = 'secret'
const CATALOG_TRACK_NAME = 'catalog'
const AUDIO_TRACK_NAME = 'audio'
const TRANSCRIPT_TRACK_NAME = 'transcript'
const AUDIO_SAMPLE_RATE = 48_000
const AUDIO_BITRATE = 64_000
const AUDIO_CHUNKS_PER_GROUP = 50
const FILTER_TYPE_NEXT_GROUP_START = 1
const OBJECT_STATUS_END_OF_GROUP = 3

type TranscriptObject = { track: string; text: string; final: boolean; at: number }

type AudioEncoderWorkerMessage =
  | {
      type: 'chunk'
      chunk: EncodedAudioChunk
      metadata: EncodedAudioChunkMetadata | undefined
      captureTimestampMicros?: number
    }
  | { type: 'bitrate'; media: 'audio'; kbps: number }
  | { type: 'configError'; media: 'audio'; reason: string }

const moqtClient = new MoqtClientWrapper()
const audioEncoderWorker = new Worker(new URL('../../utils/media/encoders/audioEncoder.ts', import.meta.url), {
  type: 'module'
})

let mediaStream: MediaStream | null = null
let transportState = new MediaTransportState()
let trackNamespace: string[] = []
let audioChunksSent = 0

function getForm(): HTMLFormElement {
  return document.getElementById('form') as HTMLFormElement
}

function requireClient(): MOQTClient {
  const client = moqtClient.getRawClient()
  if (!client) {
    throw new Error('MoQT client is not connected')
  }
  return client
}

function buildCatalogJson(namespace: string[]): string {
  return JSON.stringify({
    version: 1,
    isComplete: true,
    tracks: [
      {
        namespace: namespace.join('/'),
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
  })
}

async function sendCatalog(client: MOQTClient, trackAlias: bigint): Promise<void> {
  const payload = new TextEncoder().encode(buildCatalogJson(trackNamespace))
  await client.sendSubgroupHeader(trackAlias, 0n, 0n, 0)
  await client.sendSubgroupObject(trackAlias, 0n, 0n, 0n, undefined, payload, undefined)
}

function appendTranscript(transcript: TranscriptObject): void {
  const container = document.getElementById('transcripts') as HTMLDivElement
  const interim = container.querySelector<HTMLDivElement>('[data-interim="true"]')
  const line = interim ?? document.createElement('div')
  line.textContent = transcript.text
  line.dataset.interim = transcript.final ? 'false' : 'true'
  line.style.color = transcript.final ? '' : '#888'
  if (!interim) {
    container.appendChild(line)
  }
  container.scrollTop = container.scrollHeight
}

async function subscribeTranscripts(): Promise<void> {
  const { subscribeOk } = await moqtClient.subscribe(trackNamespace, TRANSCRIPT_TRACK_NAME, AUTH_INFO, {
    filterType: FILTER_TYPE_NEXT_GROUP_START,
    forward: true
  })
  const decoder = new TextDecoder()
  moqtClient.setOnSubgroupObjectHandler(subscribeOk.trackAlias, (_groupId, object) => {
    if (object.objectPayloadLength === 0) {
      return
    }
    appendTranscript(JSON.parse(decoder.decode(object.objectPayload)) as TranscriptObject)
  })
  setStatusText('stt-transcript-status', `Subscribed: ${trackNamespace.join('/')}/${TRANSCRIPT_TRACK_NAME}`)
}

function handleIncomingSubscribes(): void {
  moqtClient.setOnIncomingSubscribeHandler(async ({ subscribe, isSuccess, code, respondOk, respondError }) => {
    if (!isSuccess) {
      await respondError(BigInt(code), `subscribe error: code=${code}`)
      return
    }
    const requestedNamespace = (subscribe.trackNamespace ?? []).join('/')
    if (requestedNamespace !== trackNamespace.join('/')) {
      await respondError(404n, 'unknown namespace')
      return
    }
    switch (subscribe.trackName) {
      case CATALOG_TRACK_NAME: {
        const trackAlias = await respondOk(0n)
        await sendCatalog(requireClient(), trackAlias)
        return
      }
      case AUDIO_TRACK_NAME:
        await respondOk(0n)
        setStatusText('stt-capture-status', 'Server subscribed to the audio track')
        return
      default:
        await respondError(404n, 'unknown track')
    }
  })
}

/** The wasm client closes a subgroup stream when it sends EndOfGroup, and
 * readers such as moqt's TrackReader only move to the next group once the
 * current stream has ended. */
async function finishAudioGroup(client: MOQTClient, trackAliases: bigint[]): Promise<void> {
  const groupId = transportState.getAudioGroupId()
  const endObjectNumber = transportState.getAudioObjectNumber()
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
  transportState.advanceAudioGroup()
}

function startAudioEncoding(stream: MediaStream): void {
  const client = requireClient()
  const [audioTrack] = stream.getAudioTracks()
  transportState = new MediaTransportState()
  audioChunksSent = 0

  audioEncoderWorker.onmessage = async (event: MessageEvent<AudioEncoderWorkerMessage>) => {
    const data = event.data
    if (data.type === 'configError') {
      setStatusText('stt-capture-status', `Audio encoder unsupported: ${data.reason}`)
      return
    }
    if (data.type !== 'chunk') {
      return
    }
    const trackAliases = Array.from(client.getTrackSubscribers(trackNamespace, AUDIO_TRACK_NAME), (alias) =>
      BigInt(alias)
    )
    if (!trackAliases.length) {
      return
    }
    if (audioChunksSent > 0 && audioChunksSent % AUDIO_CHUNKS_PER_GROUP === 0) {
      await finishAudioGroup(client, trackAliases)
    }
    await sendAudioChunkViaMoqt({
      chunk: data.chunk,
      metadata: data.metadata,
      captureTimestampMicros: data.captureTimestampMicros,
      trackAliases,
      client,
      transportState
    })
    audioChunksSent += 1
    setStatusText('stt-send-status', String(audioChunksSent))
  }

  audioEncoderWorker.postMessage({
    type: 'config',
    config: { codec: 'opus', sampleRate: AUDIO_SAMPLE_RATE, numberOfChannels: 1, bitrate: AUDIO_BITRATE }
  })
  const processor = new MediaStreamTrackProcessor({ track: audioTrack })
  const audioStream = processor.readable
  audioEncoderWorker.postMessage({ type: 'audioStream', audioStream }, [audioStream])
}

async function start(): Promise<void> {
  const form = getForm()
  const url = (form.elements.namedItem('url') as HTMLInputElement).value
  trackNamespace = parseTrackNamespace((form.elements.namedItem('namespace') as HTMLInputElement).value)
  const startBtn = document.getElementById('startBtn') as HTMLButtonElement
  const stopBtn = document.getElementById('stopBtn') as HTMLButtonElement
  startBtn.disabled = true

  try {
    setStatusText('stt-capture-status', 'Requesting microphone')
    mediaStream = await navigator.mediaDevices.getUserMedia({ audio: true })
    setStatusText('stt-capture-status', 'Microphone ready')

    setStatusText('stt-connection-status', `Connecting: ${url}`)
    await moqtClient.connect(url)
    setStatusText('stt-connection-status', `Connected: ${url}`)

    handleIncomingSubscribes()
    await subscribeTranscripts()
    await moqtClient.publishNamespace(trackNamespace, AUTH_INFO)
    startAudioEncoding(mediaStream)
    stopBtn.disabled = false
  } catch (error) {
    setStatusText('stt-connection-status', `Failed: ${getErrorMessage(error)}`)
    console.error('[stt] start failed', error)
    await stop()
  }
}

async function stop(): Promise<void> {
  const startBtn = document.getElementById('startBtn') as HTMLButtonElement
  const stopBtn = document.getElementById('stopBtn') as HTMLButtonElement
  audioEncoderWorker.onmessage = null
  mediaStream?.getTracks().forEach((track) => track.stop())
  mediaStream = null
  await moqtClient.disconnect()
  setStatusText('stt-connection-status', 'Disconnected')
  setStatusText('stt-capture-status', 'Idle')
  setStatusText('stt-transcript-status', 'Not subscribed')
  startBtn.disabled = false
  stopBtn.disabled = true
}

configureRelayUrlControls({ defaultUrl: 'https://127.0.0.1:4433', includeRelayB: false })
document.getElementById('startBtn')!.addEventListener('click', () => void start())
document.getElementById('stopBtn')!.addEventListener('click', () => void stop())
