import { MoqSession } from 'moq-wasm'
import { connectOptions } from '../../../utils/access'
import { type LocalBroadcast, type LocalSource, createBroadcast } from 'moq-wasm/media'
import { buildVideoEncoding } from '../videoEncoding'
import { getErrorMessage, initializeMediaExamplePage, parseTrackNamespace, setStatusText } from '../../../utils/page'
import { startPageTelemetry } from '../../../utils/pageTelemetry'

const form = document.getElementById('form') as HTMLFormElement
startPageTelemetry('media-publisher', () => form.url.value)
let mediaStream: MediaStream | undefined
let session: MoqSession | undefined
let broadcast: LocalBroadcast | undefined
let camera: LocalSource | undefined
let videoMuted = false
let screenShare: LocalSource | undefined

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
  setStatusText('publisher-capture-status', 'Requesting camera')
  try {
    mediaStream = await navigator.mediaDevices.getUserMedia({
      audio: true,
      video: { width: { ideal: 1920 }, height: { ideal: 1080 } }
    })
  } catch (error) {
    setStatusText('publisher-capture-status', `getUserMedia failed: ${getErrorMessage(error)}`)
    throw error
  }
  const video = document.getElementById('video') as HTMLVideoElement
  video.srcObject = mediaStream
  const settings = mediaStream.getVideoTracks()[0]?.getSettings() ?? {}
  const dimensions =
    typeof settings.width === 'number' && typeof settings.height === 'number'
      ? `${settings.width}x${settings.height}`
      : 'unknown resolution'
  setStatusText('publisher-capture-status', `Media ready: ${dimensions}`)
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
  camera = await broadcast.addSource('camera', {
    label: 'Camera',
    stream: mediaStream,
    ...buildVideoEncoding(mediaStream),
    renditions: [{ name: 'low', label: 'Low', width: 320, height: 180, bitrate: 150_000, framerate: 15 }]
  })
  setStatusText('publisher-send-status', `Publishing started: ${namespace.join('/')}`)
}

async function toggleVideoMute(): Promise<void> {
  if (!camera) {
    setStatusText('publisher-send-status', 'Not publishing')
    return
  }
  videoMuted = !videoMuted
  await camera.setMuted('video', videoMuted)
  setStatusText('publisher-send-status', videoMuted ? 'Video muted' : 'Video unmuted')
}

async function toggleScreenShare(): Promise<void> {
  if (!broadcast) {
    setStatusText('publisher-send-status', 'Not publishing')
    return
  }
  if (screenShare) {
    await stopScreenShare()
    return
  }
  screenShare = await broadcast.addSource('screen', {
    label: 'Screen',
    display: true,
    ...buildVideoEncoding(new MediaStream())
  })
  // The browser's own stop button ends the capture without a click here.
  screenShare.listen({
    onEnded: () => {
      screenShare = undefined
      setStatusText('publisher-send-status', 'Screen share stopped')
    }
  })
  setStatusText('publisher-send-status', 'Sharing screen')
}

async function stopScreenShare(): Promise<void> {
  if (!screenShare) {
    return
  }
  screenShare = undefined
  await broadcast?.removeSource('screen')
  setStatusText('publisher-send-status', 'Screen share stopped')
}

async function close(): Promise<void> {
  videoMuted = false
  await stopScreenShare()
  await broadcast?.close()
  broadcast = undefined
  camera = undefined
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
onClick('muteVideoBtn', toggleVideoMute, 'publisher-send-status')
onClick('shareScreenBtn', toggleScreenShare, 'publisher-send-status')
onClick(
  'closeBtn',
  async () => {
    await close()
    setStatusText('publisher-connection-status', 'Disconnected')
  },
  'publisher-connection-status'
)
