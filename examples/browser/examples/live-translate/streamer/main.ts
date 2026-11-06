import { MoqSession } from 'moq-wasm'
import { type Publication, publish } from 'moq-wasm/media'
import { element, getErrorMessage, setStatusText } from '../../../utils/page'
import { startPageTelemetry } from '../../../utils/pageTelemetry'
import { setUpRelayUrlControls } from '../../../utils/relayPresets'
import { buildVideoEncoderConfig } from '../../media/videoEncoderConfig'
import { generateBroadcastId } from '../broadcastId'
import { type BroadcastAccess, openBroadcastAccess, viewerUrl } from '../liveTranslate'

const COPIED_LABEL_MS = 1_500

const broadcastId = generateBroadcastId()
const urlInput = element<HTMLInputElement>('url')
let mediaStream: MediaStream | undefined
let session: MoqSession | undefined
let publication: Publication | undefined
let access: BroadcastAccess | undefined

startPageTelemetry('live-translate-streamer', () => urlInput.value)
setUpRelayUrlControls(urlInput, element('urlPresets'))
element<HTMLElement>('broadcast-id').textContent = broadcastId
element<HTMLButtonElement>('copyBroadcastIdBtn').addEventListener(
  'click',
  (event) => void copyText(event.currentTarget as HTMLButtonElement, broadcastId)
)
element<HTMLButtonElement>('copyViewerLinkBtn').addEventListener(
  'click',
  (event) => void copyText(event.currentTarget as HTMLButtonElement, element<HTMLAnchorElement>('viewer-link').href)
)
element<HTMLButtonElement>('startBtn').addEventListener('click', () => void startBroadcast())
element<HTMLButtonElement>('stopBtn').addEventListener('click', () => void stopBroadcast())

async function startBroadcast(): Promise<void> {
  try {
    await stopBroadcast()
    access = await openBroadcastAccess(broadcastId, 'streamer')
    if (access.relayUrl) {
      urlInput.value = access.relayUrl
    }
    const url = urlInput.value.trim()
    setStatusText('streamer-status', 'Requesting camera and microphone')
    mediaStream = await navigator.mediaDevices.getUserMedia({
      audio: true,
      video: { width: { ideal: 1280 }, height: { ideal: 720 } }
    })
    element<HTMLVideoElement>('preview').srcObject = mediaStream
    session = await MoqSession.connect(url, { token: access.token })
    const namespace = access.namespace
    publication = await publish(session, namespace, mediaStream, {
      videoEncoderConfig: buildVideoEncoderConfig(mediaStream),
      onError: (error) => setStatusText('streamer-status', `Publish failed: ${getErrorMessage(error)}`)
    })
    setStatusText('streamer-status', `Broadcasting ${namespace.join('/')}`)
    showViewerLink(viewerUrl(broadcastId, url))
  } catch (error) {
    access?.release()
    access = undefined
    setStatusText('streamer-status', `Failed: ${getErrorMessage(error)}`)
  }
}

async function stopBroadcast(): Promise<void> {
  const stopped = publication !== undefined || session !== undefined
  await publication?.close()
  publication = undefined
  session?.close()
  session = undefined
  access?.release()
  access = undefined
  for (const track of mediaStream?.getTracks() ?? []) {
    track.stop()
  }
  mediaStream = undefined
  if (stopped) {
    setStatusText('streamer-status', 'Stopped')
  }
}

function showViewerLink(url: string): void {
  const link = element<HTMLAnchorElement>('viewer-link')
  link.href = url
  link.textContent = url
  element<HTMLElement>('viewer-link-section').hidden = false
}

async function copyText(button: HTMLButtonElement, text: string): Promise<void> {
  const label = (button.dataset.label ??= button.textContent ?? '')
  try {
    await navigator.clipboard.writeText(text)
    button.textContent = 'コピーしました'
    setTimeout(() => (button.textContent = label), COPIED_LABEL_MS)
  } catch (error) {
    setStatusText('streamer-status', `Copy failed: ${getErrorMessage(error)}`)
  }
}
