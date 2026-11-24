import { sdkLog } from '../core/log'
import { RequestError, RequestErrorCode } from '../core/requestError'
import type { MoqSession, MoqSubscription } from '../core/session'

const TARGET = 'moq-wasm::media::phone_call'
/// The gateway's `call` track: one JSON object `{state, reason}` per change.
const STATE_TRACK_NAME = 'call'
/// How often the `call` track is subscribed again while the gateway has not
/// published it yet.
const STATE_RETRY_MS = 500
const STATES = ['dialing', 'ringing', 'answered', 'ended'] as const

export type PhoneCallState = (typeof STATES)[number]

/// `reason` as `PhoneCallListener` describes it.
export type CallStateUpdate = { state: PhoneCallState; reason: string | undefined }

/// Reads a call's `call` track from the object of its current group on,
/// subscribing again while the gateway has not published it yet, until the
/// call ends or `close`. A track that ends after a state but without `ended`
/// (the gateway stopped) ends the call with `gateway`.
export class CallStates {
  private subscription: MoqSubscription | undefined
  private closed = false
  private seen = false

  constructor(
    private readonly session: MoqSession,
    private readonly namespace: string[],
    private readonly onUpdate: (update: CallStateUpdate) => void
  ) {}

  async run(): Promise<void> {
    while (!this.closed) {
      try {
        this.subscription = await this.session.subscribeCurrentGroup(this.namespace, STATE_TRACK_NAME)
      } catch (error) {
        if (!(error instanceof RequestError) || error.errorCode !== RequestErrorCode.TrackDoesNotExist) {
          sdkLog(this.session, 'warn', TARGET, 'call state not subscribed', { error })
        }
        await new Promise((resolve) => setTimeout(resolve, STATE_RETRY_MS))
        continue
      }
      try {
        for await (const object of this.subscription) {
          if (this.closed) {
            return
          }
          const update = parseState(new TextDecoder().decode(object.objectPayload))
          if (!update) {
            sdkLog(this.session, 'warn', TARGET, 'call state ignored')
            continue
          }
          this.seen = true
          this.onUpdate(update)
          if (update.state === 'ended') {
            return
          }
        }
      } catch (error) {
        sdkLog(this.session, 'warn', TARGET, 'call state not read', { error })
      }
      if (this.seen && !this.closed) {
        this.onUpdate({ state: 'ended', reason: 'gateway' })
        return
      }
    }
  }

  async close(): Promise<void> {
    this.closed = true
    await this.subscription?.close().catch((error: unknown) => {
      sdkLog(this.session, 'warn', TARGET, 'call state not unsubscribed', { error })
    })
  }
}

/// Parses one object of the `call` track; `undefined` unless it names a known state.
export function parseState(text: string): CallStateUpdate | undefined {
  let value: unknown
  try {
    value = JSON.parse(text)
  } catch {
    return undefined
  }
  if (typeof value !== 'object' || value === null) {
    return undefined
  }
  const { state, reason } = value as { state?: unknown; reason?: unknown }
  if (typeof state !== 'string' || !(STATES as readonly string[]).includes(state)) {
    return undefined
  }
  return { state: state as PhoneCallState, reason: typeof reason === 'string' ? reason : undefined }
}
