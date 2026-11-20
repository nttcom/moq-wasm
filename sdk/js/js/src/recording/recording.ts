import { sdkLog } from '../core/log'
import type { MoqSession } from '../core/session'

const TARGET = 'moq-wasm::recording'
/// Longer than the 30 s a FETCH sent to the previous replay can take: the relay waits 10 s for
/// its FETCH_OK and 20 s to fill it.
const OVERLAP_MS = 60_000

export type OpenRecordingOptions = {
  /// Sent as `Authorization: Bearer`. A function is called before each `publish` request,
  /// the renewals included.
  token?: string | (() => Promise<string>)
}

/// The recorder refused `publish`: 404 for an unknown recording, 409 for one not finished or
/// not in the moq format, 401 or 403 for the token or a room it cannot replay, 503 when it
/// replays too many recordings or cannot verify the token.
export class RecorderError extends Error {
  constructor(
    readonly status: number,
    message: string
  ) {
    super(message)
  }
}

/// The page's `fetch` and timers; tests pass fakes.
export type RecordingEnvironment = {
  fetch(url: string, init: RequestInit): Promise<Response>
  setTimeout(action: () => void, delayMs: number): number
  clearTimeout(timer: number | undefined): void
}

const PAGE: RecordingEnvironment = {
  fetch: (url, init) => fetch(url, init),
  setTimeout: (action, delayMs) => window.setTimeout(action, delayMs),
  clearTimeout: (timer) => window.clearTimeout(timer)
}

type PublishRequest = { url: string; token: OpenRecordingOptions['token'] }

/// Asks the recorder to replay recording `id` to its relay and holds the answer, which keeps
/// the replay running, until `close()` or until `session` closes. Every `renew_after_seconds`
/// the recorder names, it opens a new answer and closes the previous one a minute later. When
/// an answer ends on its own, it opens a new one at once. Rejects with a `RecorderError` when
/// the recorder refuses, and with another `Error` when the request fails or the answer does not
/// describe the replay.
export function openRecording(
  session: MoqSession,
  recorderUrl: string,
  id: string,
  options: OpenRecordingOptions = {}
): Promise<Recording> {
  const url = `${recorderUrl.replace(/\/+$/, '')}/recordings/${encodeURIComponent(id)}/publish`
  return Recording.open(session, { url, token: options.token }, PAGE)
}

export class Recording {
  /// Resolves once no answer is held: with `undefined` after `close()` or once the session
  /// closed, with the error when an answer ended and no new one could be opened.
  readonly closed: Promise<Error | undefined>
  private resolveClosed!: (error: Error | undefined) => void
  private isClosed = false
  private opening: AbortController | undefined
  private renewal: number | undefined
  private readonly retiring = new Map<Answer, number>()

  private constructor(
    /// The namespace prefix of the replay's broadcasts, for `session.watchNamespaces`.
    readonly prefix: string[],
    /// When the recording started, in Unix milliseconds: the replay's LOC Timestamps are media
    /// time since then, so a time of day is this plus a position.
    readonly startedAtMs: number,
    private current: Answer,
    private readonly request: PublishRequest,
    private readonly environment: RecordingEnvironment,
    /// The session the replay is watched on, whose token the recording's logs go with.
    private readonly session: Pick<MoqSession, 'closed'>
  ) {
    this.closed = new Promise((resolve) => {
      this.resolveClosed = resolve
    })
    this.hold(current)
    this.scheduleRenewal()
  }

  static async open(
    session: Pick<MoqSession, 'closed'>,
    request: PublishRequest,
    environment: RecordingEnvironment
  ): Promise<Recording> {
    const { prefix, startedAtMs, answer } = await openAnswer(request, environment, new AbortController())
    const recording = new Recording(prefix, startedAtMs, answer, request, environment, session)
    void session.closed.then(() => recording.close())
    return recording
  }

  close(): void {
    this.end(undefined)
  }

  private hold(answer: Answer): void {
    void answer.ended.then(() => {
      if (answer === this.current && !this.isClosed && !this.opening) {
        sdkLog(this.session, 'warn', TARGET, 'recording answer ended, opening a new one', { url: this.request.url })
        void this.renew()
      }
    })
  }

  private scheduleRenewal(): void {
    this.renewal = this.environment.setTimeout(() => void this.renew(), this.current.renewAfterMs)
  }

  private async renew(): Promise<void> {
    this.environment.clearTimeout(this.renewal)
    const opening = (this.opening = new AbortController())
    let next: Answer
    try {
      next = (await openAnswer(this.request, this.environment, opening)).answer
    } catch (error) {
      if (this.isClosed) {
        return
      }
      this.opening = undefined
      if (this.current.isEnded) {
        this.end(error instanceof Error ? error : new Error(String(error)))
      } else {
        sdkLog(this.session, 'warn', TARGET, 'recording answer renewal failed', { url: this.request.url, error })
      }
      return
    }
    if (this.isClosed) {
      return
    }
    this.opening = undefined
    const previous = this.current
    this.current = next
    this.hold(next)
    this.scheduleRenewal()
    const retirement = this.environment.setTimeout(() => {
      this.retiring.delete(previous)
      previous.close()
    }, OVERLAP_MS)
    this.retiring.set(previous, retirement)
  }

  private end(error: Error | undefined): void {
    if (this.isClosed) {
      return
    }
    this.isClosed = true
    this.environment.clearTimeout(this.renewal)
    this.opening?.abort()
    for (const [answer, retirement] of this.retiring) {
      this.environment.clearTimeout(retirement)
      answer.close()
    }
    this.retiring.clear()
    this.current.close()
    this.resolveClosed(error)
  }
}

/// The response to one `publish` request.
class Answer {
  isEnded = false
  readonly ended: Promise<void>

  constructor(
    readonly renewAfterMs: number,
    private readonly controller: AbortController,
    reader: ReadableStreamDefaultReader<Uint8Array>
  ) {
    this.ended = readToEnd(reader).then(() => {
      this.isEnded = true
    })
  }

  close(): void {
    this.controller.abort()
  }
}

/// The answer is NDJSON: the replay's description, then an empty line every 15 s.
async function openAnswer(
  request: PublishRequest,
  environment: RecordingEnvironment,
  controller: AbortController
): Promise<{ prefix: string[]; startedAtMs: number; answer: Answer }> {
  const token = typeof request.token === 'function' ? await request.token() : request.token
  const response = await environment.fetch(request.url, {
    method: 'POST',
    headers: token ? { Authorization: `Bearer ${token}` } : {},
    signal: controller.signal
  })
  if (!response.ok) {
    const body = await response.json().catch(() => ({}))
    throw new RecorderError(
      response.status,
      typeof body.error === 'string' ? body.error : `the recorder answered ${response.status}`
    )
  }
  const reader = response.body!.getReader()
  try {
    const { prefix, renewAfterMs, startedAtMs } = parseDescription(await readLine(reader))
    return { prefix, startedAtMs, answer: new Answer(renewAfterMs, controller, reader) }
  } catch (error) {
    controller.abort()
    throw error
  }
}

async function readLine(reader: ReadableStreamDefaultReader<Uint8Array>): Promise<string> {
  const decoder = new TextDecoder()
  let text = ''
  while (!text.includes('\n')) {
    const { value, done } = await reader.read()
    if (done) {
      throw new Error('the recorder ended the answer before describing the replay')
    }
    text += decoder.decode(value, { stream: true })
  }
  return text.slice(0, text.indexOf('\n'))
}

function parseDescription(line: string): { prefix: string[]; renewAfterMs: number; startedAtMs: number } {
  const {
    namespace_prefix: prefix,
    renew_after_seconds: renewAfterSeconds,
    started_at_ms: startedAtMs
  } = JSON.parse(line)
  if (typeof prefix !== 'string' || !(renewAfterSeconds > 0) || typeof startedAtMs !== 'number') {
    throw new Error(`the recorder described the replay as ${line}`)
  }
  return { prefix: prefix.split('/'), renewAfterMs: renewAfterSeconds * 1_000, startedAtMs }
}

/// An answer the page aborted or the network broke has ended too.
async function readToEnd(reader: ReadableStreamDefaultReader<Uint8Array>): Promise<void> {
  try {
    while (!(await reader.read()).done) {}
  } catch {}
}
