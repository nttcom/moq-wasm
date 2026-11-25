import type { TrackWriter } from '../../pkg/moqt'
import type { MoqSession } from '../core/session'
import { CallStates, type CallStateUpdate, type PhoneCallState } from './callStates'
import { callPage } from './internal/callPage'
import { type LocalBroadcast, type TrackInput, createBroadcast } from './localBroadcast'
import { RemoteBroadcast } from './remoteBroadcast'

/// The page's `control` track: one JSON object per command, each a group of
/// its own.
const CONTROL_TRACK_NAME = 'control'
const MICROPHONE_SOURCE_NAME = 'microphone'
const DTMF_DIGITS = /^[0-9*#A-D]+$/

/// `reason` is set when `state` is `ended`: the trunk's final SIP status
/// (`486`, `603`, …), `hangup`, `timeout`, `limit` or `gateway`.
/// What the listener throws is reported with `reportError`, as an exception of an event
/// listener is; the other listeners are still told and an ended call still
/// stops the microphone and the phone's audio.
export type PhoneCallListener = (state: PhoneCallState, reason: string | undefined) => void

export type PhoneCallOptions = {
  /// The gateway's broadcast, `<room>/phone/<call_id>`, as the app server got it.
  call: string[]
  /// The broadcast the page publishes for the call: the `peer` the app server
  /// named to the gateway.
  local: string[]
  /// The microphone the phone hears: a device the SDK captures (`true` takes
  /// the default one), or a track of the page.
  audio: TrackInput
  /// Plays the phone through the page's context, as
  /// `RemoteBroadcastOptions.audioContext` does.
  audioContext?: AudioContext
}

/// A call between the page and a phone, which the page's app server placed or
/// answered through the SIP gateway. The page publishes its microphone and a
/// `control` track in `local`, plays the phone's audio and follows the call's
/// state on the gateway's `call` track.
export class PhoneCall {
  private readonly listeners = new Set<PhoneCallListener>()
  private current: CallStateUpdate | undefined
  private readonly states: CallStates
  private finished = false
  private finishing: Promise<void> | undefined

  private constructor(
    session: MoqSession,
    call: string[],
    private readonly local: LocalBroadcast,
    private readonly control: TrackWriter,
    /// Plays the phone's audio; the call disposes of it once it ends.
    readonly phone: RemoteBroadcast
  ) {
    this.states = new CallStates(session, call, (update) => this.update(update))
    void this.states.run()
  }

  /// Publishes `local` with the microphone and its `control` track, and starts
  /// playing and following `call`. Rejects, after closing `local`, when the
  /// microphone cannot be captured or `control` cannot be published.
  static async join(session: MoqSession, options: PhoneCallOptions): Promise<PhoneCall> {
    const local = await createBroadcast(session, options.local)
    try {
      const control = await session.publishTrack(options.local, CONTROL_TRACK_NAME)
      await local.addSource(MICROPHONE_SOURCE_NAME, { audio: options.audio })
      const phone = new RemoteBroadcast(session, options.call, {
        audioContext: options.audioContext,
        pauseHiddenVideo: false
      })
      return new PhoneCall(session, options.call, local, control, phone)
    } catch (error) {
      await local.close()
      throw error
    }
  }

  /// Tells the listener the newest state at once, if there is one. Returns the
  /// function that removes it.
  listen(listener: PhoneCallListener): () => void {
    this.listeners.add(listener)
    if (this.current) {
      const { state, reason } = this.current
      callPage(() => listener(state, reason))
    }
    return () => this.listeners.delete(listener)
  }

  /// Sends each digit (`0`-`9`, `*`, `#`, `A`-`D`) to the phone as a DTMF
  /// event. Rejects for any other character and once the call has ended.
  async sendDtmf(digits: string): Promise<void> {
    if (!DTMF_DIGITS.test(digits)) {
      throw new Error(`${JSON.stringify(digits)} is not a sequence of DTMF digits`)
    }
    if (this.finished) {
      throw new Error('the call has ended')
    }
    for (const digit of digits) {
      await this.send({ event: 'dtmf', digit })
    }
  }

  /// Ends the call: the gateway hangs up the phone and the call ends with
  /// `hangup`. Resolves once the microphone and the phone's audio have stopped.
  async hangUp(): Promise<void> {
    if (this.finished) {
      return
    }
    await this.send({ event: 'hangup' })
    this.update({ state: 'ended', reason: 'hangup' })
    await this.finish()
  }

  private async send(command: object): Promise<void> {
    await this.control.writeGroup(new TextEncoder().encode(JSON.stringify(command)))
  }

  private update(update: CallStateUpdate): void {
    this.current = update
    for (const listener of this.listeners) {
      callPage(() => listener(update.state, update.reason))
    }
    if (update.state === 'ended') {
      void this.finish()
    }
  }

  private finish(): Promise<void> {
    this.finished = true
    this.finishing ??= Promise.all([this.local.close(), this.phone.dispose(), this.states.close()]).then(() => {})
    return this.finishing
  }
}
