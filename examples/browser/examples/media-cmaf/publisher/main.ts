import { MoqSession } from 'moq-wasm'
import { type LocalBroadcast, createBroadcast } from 'moq-wasm/media'
import { connectOptions } from '../../../utils/access'
import { buildVideoEncoding } from '../../media/videoEncoding'
import {
  element,
  getErrorMessage,
  initializeMediaExamplePage,
  parseTrackNamespace,
  setStatusText
} from '../../../utils/page'
import { startPageTelemetry } from '../../../utils/pageTelemetry'

const form = document.getElementById('form') as HTMLFormElement
startPageTelemetry('media-cmaf-publisher', () => form.url.value)
let mediaStream: MediaStream | undefined
let session: MoqSession | undefined
let broadcast: LocalBroadcast | undefined

function initializeStatuses(): void {
  setStatusText('publisher-connection-status', 'Not connected')
  setStatusText('publisher-capture-status', 'Capture idle')
  setStatusText('publisher-send-status', 'Not publishing')
}

async function connect(): Promise<void> {
  await close()
  const url = form.url.value
  session = await MoqSession.connect(url, connectOptions())
  setStatusText('publisher-connection-status', `Connected: ${url}`)
}

async function startCapture(): Promise<void> {
  mediaStream = await navigator.mediaDevices.getUserMedia({
    audio: true,
    video: { width: { ideal: 1280 }, height: { ideal: 720 } }
  })
  const video = document.getElementById('video') as HTMLVideoElement
  video.srcObject = mediaStream
  element<HTMLElement>('preview-stage').dataset.started = ''
  setStatusText('publisher-capture-status', 'Media ready')
}

async function startPublishing(): Promise<void> {
  if (!session || !mediaStream) {
    setStatusText('publisher-send-status', session ? 'Media stream is not ready' : 'Not connected')
    return
  }
  if (broadcast) {
    setStatusText('publisher-send-status', 'Publishing already started')
    return
  }
  const namespace = parseTrackNamespace(form['publish-track-namespace'].value)
  broadcast = await createBroadcast(session, namespace, {
    onError: (error) => setStatusText('publisher-send-status', `Publish failed: ${getErrorMessage(error)}`)
  })
  const { encoding, main } = buildVideoEncoding(mediaStream)
  await broadcast.addSource('camera', {
    label: 'Camera',
    stream: mediaStream,
    encoding: { ...encoding, keyframeIntervalSeconds: Number(form['keyframe-interval'].value) },
    main,
    cmaf: true
  })
  setStatusText('publisher-send-status', `Publishing started: ${namespace.join('/')}`)
}

async function close(): Promise<void> {
  await broadcast?.close()
  broadcast = undefined
  session?.close()
  session = undefined
  initializeStatuses()
}

function onClick(id: string, action: () => Promise<void>, statusId: string): void {
  document.getElementById(id)?.addEventListener('click', () => {
    action().catch((error: unknown) => {
      console.error(error)
      setStatusText(statusId, `Failed: ${getErrorMessage(error)}`)
    })
  })
}

initializeMediaExamplePage('publish-track-namespace')
initializeStatuses()
onClick('connectBtn', connect, 'publisher-connection-status')
onClick('startGetUserMediaBtn', startCapture, 'publisher-capture-status')
onClick('publishBtn', startPublishing, 'publisher-send-status')
onClick(
  'closeBtn',
  async () => {
    await close()
    setStatusText('publisher-connection-status', 'Disconnected')
  },
  'publisher-connection-status'
)
