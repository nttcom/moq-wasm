import { MoqSession } from '@moqt/session'
import { type Publication, publish } from '@publish/publish'
import { getResolvedMediaVideoCodec } from '../catalog'
import { getMediaVideoEncodingOverrides } from '../common'
import { getErrorMessage, initializeMediaExamplePage, parseTrackNamespace, setStatusText } from '../../../utils/page'

const form = document.getElementById('form') as HTMLFormElement
let mediaStream: MediaStream | undefined
let session: MoqSession | undefined
let publication: Publication | undefined
let videoMuted = false

function initializeStatuses(): void {
  setStatusText('publisher-connection-status', 'Not connected')
  setStatusText('publisher-capture-status', 'Capture idle')
  setStatusText('publisher-send-status', 'Not publishing')
}

async function connect(): Promise<void> {
  await close()
  const url = form.url.value
  session = await MoqSession.connect(url)
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
  if (publication) {
    setStatusText('publisher-send-status', 'Publishing already started')
    return
  }
  const namespace = parseTrackNamespace(form['publish-track-namespace'].value)
  const settings = mediaStream.getVideoTracks()[0]?.getSettings() ?? {}
  const { hardwareAcceleration } = getMediaVideoEncodingOverrides()
  publication = await publish(session, namespace, mediaStream, {
    videoEncoderConfig: {
      codec: getResolvedMediaVideoCodec(),
      width: typeof settings.width === 'number' ? settings.width : 1280,
      height: typeof settings.height === 'number' ? settings.height : 720,
      bitrate: 1_000_000,
      framerate: typeof settings.frameRate === 'number' ? settings.frameRate : 30,
      hardwareAcceleration
    },
    renditions: [{ name: 'low', label: 'Low', width: 320, height: 180, bitrate: 150_000 }],
    onError: (error) => setStatusText('publisher-send-status', `Publish failed: ${getErrorMessage(error)}`)
  })
  setStatusText('publisher-send-status', `Publishing started: ${namespace.join('/')}`)
}

async function toggleVideoMute(): Promise<void> {
  if (!publication) {
    setStatusText('publisher-send-status', 'Not publishing')
    return
  }
  videoMuted = !videoMuted
  await publication.setMuted('video', videoMuted)
  setStatusText('publisher-send-status', videoMuted ? 'Video muted' : 'Video unmuted')
}

async function close(): Promise<void> {
  videoMuted = false
  await publication?.close()
  publication = undefined
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
onClick(
  'closeBtn',
  async () => {
    await close()
    setStatusText('publisher-connection-status', 'Disconnected')
  },
  'publisher-connection-status'
)
