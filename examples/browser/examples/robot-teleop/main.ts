import { MoqSession, type MoqSubscription, type TrackWriter } from 'moq-wasm'
import { createBroadcast } from 'moq-wasm/media'
import { connectOptions, withNamespaceRoot } from '../../utils/access'
import { setUpRelayUrlControls } from '../../utils/relayPicker'
import { element, getErrorMessage } from '../../utils/page'
import { startPageTelemetry } from '../../utils/pageTelemetry'
import { type RobotState, decodeControl, decodeState, encodeAction, isDriveKey, velocityOf } from './teleopWire'

const SEND_INTERVAL_MS = 50
const SPEED = 0.5
/// The robot's tracks' priorities and the action age it accepts
/// (docs/architecture/robotics/architecture.md, Priority and delivery).
const ACTION_PRIORITY = 0
const CONTROL_PRIORITY = 0
const STATE_PRIORITY = 32
const MAX_ACTION_AGE_MS = 300
const RESUBSCRIBE_DELAY_MS = 1_000
const ACTION_EVENT_TYPE = 'io.skyway.research.moqt.robotics.action.v1'

const urlInput = element<HTMLInputElement>('url')
const roomInput = element<HTMLInputElement>('room')
const connectButton = element<HTMLButtonElement>('connectBtn')
const pad = element<HTMLCanvasElement>('pad')

startPageTelemetry('robot-teleop', () => urlInput.value.trim())
setUpRelayUrlControls(urlInput, element('urlPresets'))

const held = new Set<string>()
let state: RobotState | undefined

window.addEventListener('keydown', (event) => {
  if (isDriveKey(event.code) && !(event.target instanceof HTMLInputElement)) {
    held.add(event.code)
    event.preventDefault()
  }
})
window.addEventListener('keyup', (event) => held.delete(event.code))
window.addEventListener('blur', () => held.clear())
connectButton.addEventListener('click', () => void connect())
requestAnimationFrame(draw)

function setStatus(text: string): void {
  element('status').textContent = text
}

async function connect(): Promise<void> {
  connectButton.disabled = true
  setStatus('接続中…')
  try {
    const session = await MoqSession.connect(urlInput.value.trim(), connectOptions())
    const room = withNamespaceRoot(roomInput.value.trim().split('/'))
    const operator = `operators/${crypto.randomUUID().slice(0, 8)}`
    const namespace = [...room, ...operator.split('/')]
    // Listed in the catalog, so a room recording keeps the actions too.
    await createBroadcast(session, namespace, {
      eventTimelines: [{ name: 'action', eventType: ACTION_EVENT_TYPE, priority: ACTION_PRIORITY }]
    })
    const writer = await session.publishTrack(namespace, 'action', {
      publish: true,
      priority: ACTION_PRIORITY,
      retentionMs: 0
    })
    element('operator').textContent = operator
    const robot = [...room, 'robot']
    void follow(session, robot, 'state', (payload) => {
      state = decodeState(payload) ?? state
      if (state) {
        element('position').textContent = `x ${state.x.toFixed(2)} / y ${state.y.toFixed(2)}`
      }
    })
    void follow(session, robot, 'control', (payload) => {
      const control = decodeControl(payload)
      if (control) {
        element('driver').textContent = `${control.driver ?? 'なし'} (${control.reason})`
      }
    })
    startSending(session, writer)
    setStatus('接続しました')
  } catch (error) {
    setStatus(`接続できません: ${getErrorMessage(error)}`)
    connectButton.disabled = false
  }
}

async function follow(
  session: MoqSession,
  namespace: string[],
  track: 'state' | 'control',
  onObject: (payload: Uint8Array) => void
): Promise<void> {
  while (session.isOpen) {
    let subscription: MoqSubscription | undefined
    try {
      subscription = await session.subscribeCurrentGroup(namespace, track, {
        priority: track === 'state' ? STATE_PRIORITY : CONTROL_PRIORITY,
        deliveryTimeoutMs: MAX_ACTION_AGE_MS
      })
      for await (const object of subscription) {
        onObject(object.objectPayload)
      }
    } catch {
      // The robot has not published the track yet.
    }
    await subscription?.close()
    await new Promise((resolve) => setTimeout(resolve, RESUBSCRIBE_DELAY_MS))
  }
}

/// Sends no stop action on release: the robot holds once the actions go stale.
function startSending(session: MoqSession, writer: TrackWriter): void {
  let seq = 0
  const timer = window.setInterval(() => {
    if (!session.isOpen) {
      window.clearInterval(timer)
      return
    }
    if (held.size === 0 || !state) {
      return
    }
    seq += 1
    const payload = encodeAction(
      { seq, basis: state.t, dtMs: SEND_INTERVAL_MS, velocity: velocityOf(held, SPEED) },
      Date.now()
    )
    void writer.writeGroup(payload).catch((error) => setStatus(`送れません: ${getErrorMessage(error)}`))
  }, SEND_INTERVAL_MS)
}

function draw(): void {
  const context = pad.getContext('2d')!
  const { width, height } = pad
  context.clearRect(0, 0, width, height)
  context.strokeStyle = '#ccc'
  context.strokeRect(0.5, 0.5, width - 1, height - 1)
  if (state) {
    const x = ((state.x + 1) / 2) * width
    const y = ((1 - state.y) / 2) * height
    context.fillStyle = '#0f766e'
    context.beginPath()
    context.arc(x, y, 10, 0, Math.PI * 2)
    context.fill()
  }
  requestAnimationFrame(draw)
}
