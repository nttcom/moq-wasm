import { type IncomingSubscribeContext, MoqtClientWrapper, RequestErrorCode } from '@moqt/moqtClient'
import type { MOQTClient, SubgroupObjectMessage } from '../../pkg/moqt_client_wasm'
import { OBJECT_STATUS_END_OF_GROUP } from '../../utils/media/objectStatus'
import { appendCloudRelayPresetButtons, configureRelayUrlControls } from '../../utils/relayPresets'
import { type StatusState, element, getErrorMessage, setStatus } from '../media/common'

const CAMERA_NAMESPACE = ['anon', 'moq-camera-detection', 'camera']
const DETECTOR_NAMESPACE = ['anon', 'moq-camera-detection', 'detector']
const VIDEO_TRACK = 'video'
const EVENT_TIMELINE_TRACK = 'eventtimeline'
const AUTH_INFO = ''
const SUBGROUP_ID = 0n
const PUBLISHER_PRIORITY = 0
const LARGEST_OBJECT_FILTER = 0x2
const VIDEO_ENCODER_CONFIG = {
  codec: 'avc1.42e01f',
  width: 640,
  height: 480,
  bitrate: 500_000,
  framerate: 30,
  hardwareAcceleration: 'prefer-software'
}

class GopSender {
  /// The relay isolates a track whose publisher repeats a location, so a
  /// publisher that comes back must not restart its group ids at zero.
  private nextGroupId = BigInt(Date.now()) * 1_000n
  private groupId: bigint | undefined
  private nextObjectId = 0n

  constructor(
    private readonly client: MOQTClient,
    readonly requestId: bigint,
    private readonly trackAlias: bigint
  ) {}

  /// Each keyframe opens a group, so the detector, which starts a decoder at every
  /// group, always starts from a decodable picture.
  async send(chunk: EncodedVideoChunk): Promise<void> {
    if (chunk.type === 'key') {
      await this.closeGroup()
      this.groupId = this.nextGroupId++
      this.nextObjectId = 0n
      await this.client.sendSubgroupHeader(this.trackAlias, this.groupId, SUBGROUP_ID, PUBLISHER_PRIORITY)
    }
    if (this.groupId === undefined) {
      return
    }
    const payload = new Uint8Array(chunk.byteLength)
    chunk.copyTo(payload)
    await this.client.sendSubgroupObject(
      this.trackAlias,
      this.groupId,
      SUBGROUP_ID,
      this.nextObjectId,
      undefined,
      payload,
      undefined
    )
    this.nextObjectId += 1n
  }

  private async closeGroup(): Promise<void> {
    if (this.groupId === undefined) {
      return
    }
    await this.client.sendSubgroupObject(
      this.trackAlias,
      this.groupId,
      SUBGROUP_ID,
      this.nextObjectId,
      OBJECT_STATUS_END_OF_GROUP,
      new Uint8Array(0),
      undefined
    )
  }
}

const session = new MoqtClientWrapper()
let videoSender: GopSender | undefined
let mediaStream: MediaStream | undefined
let encoderWorker: Worker | undefined
let sendQueue = Promise.resolve()

const urlInput = element<HTMLInputElement>('url')
const joinButton = element<HTMLButtonElement>('joinBtn')
const leaveButton = element<HTMLButtonElement>('leaveBtn')
const preview = element<HTMLVideoElement>('preview')
const verdictLabel = element<HTMLSpanElement>('verdict')

appendCloudRelayPresetButtons(element('urlPresets'))
configureRelayUrlControls()

joinButton.addEventListener('click', () => void join())
leaveButton.addEventListener('click', () => void leave())

async function join(): Promise<void> {
  joinButton.disabled = true
  setStatus('connection-status', '接続中', 'review')
  try {
    mediaStream = await navigator.mediaDevices.getUserMedia({
      video: { width: VIDEO_ENCODER_CONFIG.width, height: VIDEO_ENCODER_CONFIG.height },
      audio: false
    })
    preview.srcObject = mediaStream
    await session.connect(urlInput.value.trim())
    session.setOnConnectionClosedHandler(() => resetSession('切断されました', 'error'))
    session.setOnIncomingSubscribeHandler(acceptVideoSubscriber)
    session.setOnIncomingUnsubscribeHandler((requestId) => {
      if (videoSender?.requestId === requestId) {
        setVideoSender(undefined)
      }
    })
    session.setOnPublishNamespaceHandler(async ({ publishNamespace, respondOk }) => {
      await respondOk()
      if (sameNamespace(publishNamespace.trackNamespace, DETECTOR_NAMESPACE)) {
        await subscribeVerdicts()
      }
    })
    await session.publishNamespace(CAMERA_NAMESPACE, AUTH_INFO)
    await session.subscribeNamespace(DETECTOR_NAMESPACE, AUTH_INFO)
    startEncoder(mediaStream)
    setStatus('connection-status', '接続済み', 'ok')
    leaveButton.disabled = false
  } catch (error) {
    await session.disconnect()
    resetSession(`接続に失敗しました: ${getErrorMessage(error)}`, 'error')
  }
}

async function leave(): Promise<void> {
  await session.disconnect()
  resetSession('未接続', 'idle')
}

function resetSession(connectionText: string, connectionState: StatusState): void {
  setVideoSender(undefined)
  for (const track of mediaStream?.getTracks() ?? []) {
    track.stop()
  }
  mediaStream = undefined
  preview.srcObject = null
  setStatus('connection-status', connectionText, connectionState)
  showVerdict(undefined)
  joinButton.disabled = false
  leaveButton.disabled = true
}

function startEncoder(stream: MediaStream): void {
  encoderWorker ??= new Worker(new URL('../../utils/media/encoders/videoEncoder.ts', import.meta.url), {
    type: 'module'
  })
  encoderWorker.onmessage = (event: MessageEvent<{ type: string; chunk: EncodedVideoChunk }>) => {
    if (event.data.type === 'chunk') {
      const { chunk } = event.data
      sendQueue = sendQueue
        .then(() => videoSender?.send(chunk))
        .catch((error) => console.error('[moq-camera-detection] video send failed', error))
    }
  }
  encoderWorker.postMessage({ type: 'encoderConfig', config: VIDEO_ENCODER_CONFIG })
  const [videoTrack] = stream.getVideoTracks()
  const videoStream = new MediaStreamTrackProcessor({ track: videoTrack }).readable
  encoderWorker.postMessage({ type: 'videoStream', videoStream }, [videoStream])
}

async function acceptVideoSubscriber({
  subscribe,
  isSuccess,
  code,
  respondOk,
  respondError
}: IncomingSubscribeContext): Promise<void> {
  if (!isSuccess) {
    await respondError(BigInt(code), 'subscribe rejected')
    return
  }
  if (!sameNamespace(subscribe.trackNamespace, CAMERA_NAMESPACE) || subscribe.trackName !== VIDEO_TRACK) {
    await respondError(RequestErrorCode.TrackDoesNotExist, 'unknown track')
    return
  }
  const trackAlias = await respondOk()
  const client = session.getRawClient()
  if (client) {
    setVideoSender(new GopSender(client, subscribe.requestId, trackAlias))
    encoderWorker?.postMessage({ type: 'forceKeyframe' })
  }
}

function setVideoSender(sender: GopSender | undefined): void {
  videoSender = sender
  setStatus('detector-status', sender ? '判定サーバ接続済み' : '判定サーバ未接続', sender ? 'ok' : 'idle')
}

async function subscribeVerdicts(): Promise<void> {
  try {
    const { subscribeOk } = await session.subscribe(DETECTOR_NAMESPACE, EVENT_TIMELINE_TRACK, AUTH_INFO, {
      filterType: LARGEST_OBJECT_FILTER,
      forward: true
    })
    session.setOnSubgroupObjectHandler(subscribeOk.trackAlias, (_groupId, object) => applyVerdicts(object))
  } catch (error) {
    console.warn('[moq-camera-detection] event timeline subscribe failed', error)
  }
}

/// draft-ietf-moq-msf-01 §8.1: each event timeline object is a JSON array of records; the
/// newest one, last in the array, carries the latest verdict.
function applyVerdicts(object: SubgroupObjectMessage): void {
  if (object.objectStatus !== undefined) {
    return
  }
  const person = JSON.parse(new TextDecoder().decode(new Uint8Array(object.objectPayload))).at(-1)?.data?.person
  if (typeof person === 'boolean' || person === null) {
    showVerdict(person)
  }
}

function showVerdict(person: boolean | null | undefined): void {
  verdictLabel.dataset.verdict = person === undefined ? 'pending' : person === null ? 'unknown' : String(person)
  verdictLabel.textContent =
    person === undefined
      ? '判定待ち'
      : person === null
        ? '判定できません'
        : person
          ? '人が映っています'
          : '人は映っていません'
}

function sameNamespace(left: string[], right: string[]): boolean {
  return left.join('/') === right.join('/')
}
