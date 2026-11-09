import { expect, test } from '@playwright/test'
import type { CloseInfo } from '../core/session'
import { RecorderError, Recording, type RecordingEnvironment } from './recording'

const PUBLISH_URL = 'https://recorder.example/recordings/rec-1/publish'
const RENEW_AFTER_SECONDS = 2_700
const RENEW_AFTER_MS = RENEW_AFTER_SECONDS * 1_000
const OVERLAP_MS = 60_000

class FakeAnswer {
  isClosed = false
  readonly response = Promise.withResolvers<Response>()
  private body: ReadableStreamDefaultController<Uint8Array> | undefined

  constructor(
    readonly url: string,
    readonly init: RequestInit
  ) {
    init.signal?.addEventListener('abort', () => {
      this.isClosed = true
      const error = new DOMException('aborted', 'AbortError')
      this.response.reject(error)
      this.body?.error(error)
    })
  }

  get authorization(): string | null {
    return new Headers(this.init.headers).get('authorization')
  }

  describe(
    line = `{"namespace_prefix":"app/room/recordings/rec-1","renew_after_seconds":${RENEW_AFTER_SECONDS},"started_at_ms":1700000000000}`
  ): void {
    const encoder = new TextEncoder()
    const stream = new ReadableStream<Uint8Array>({
      start: (controller) => {
        this.body = controller
      }
    })
    // Two chunks, so `readLine` must join them; the empty line after the description is a keepalive.
    this.body!.enqueue(encoder.encode(line.slice(0, 10)))
    this.body!.enqueue(encoder.encode(`${line.slice(10)}\n\n`))
    this.response.resolve(new Response(stream))
  }

  refuse(status: number, error: string): void {
    this.response.resolve(Response.json({ error }, { status }))
  }

  end(): void {
    this.body!.close()
  }
}

class FakeRecorder {
  readonly answers: FakeAnswer[] = []
  private nowMs = 0
  private nextTimer = 1
  private timers: { id: number; dueMs: number; action: () => void }[] = []

  readonly environment: RecordingEnvironment = {
    fetch: (url, init) => {
      const answer = new FakeAnswer(url, init)
      this.answers.push(answer)
      return answer.response.promise
    },
    setTimeout: (action, delayMs) => {
      const id = this.nextTimer++
      this.timers.push({ id, dueMs: this.nowMs + delayMs, action })
      return id
    },
    clearTimeout: (id) => {
      this.timers = this.timers.filter((timer) => timer.id !== id)
    }
  }

  get pendingTimers(): number {
    return this.timers.length
  }

  get openAnswers(): number {
    return this.answers.filter((answer) => !answer.isClosed).length
  }

  /// Sends the first request, `answers[0]`, and leaves it unanswered.
  async start(
    token?: string | (() => Promise<string>),
    session = { closed: new Promise<CloseInfo>(() => {}) }
  ): Promise<{ opening: Promise<Recording> }> {
    const opening = Recording.open(session, { url: PUBLISH_URL, token }, this.environment)
    await settle()
    return { opening }
  }

  async open(...options: Parameters<FakeRecorder['start']>): Promise<Recording> {
    const { opening } = await this.start(...options)
    this.answers[0].describe()
    return opening
  }

  async advanceBy(ms: number): Promise<void> {
    const targetMs = this.nowMs + ms
    for (;;) {
      await settle()
      const [due] = this.timers.filter((timer) => timer.dueMs <= targetMs).sort((a, b) => a.dueMs - b.dueMs)
      if (!due) {
        break
      }
      this.timers = this.timers.filter((timer) => timer !== due)
      this.nowMs = due.dueMs
      due.action()
    }
    this.nowMs = targetMs
  }
}

/// Lets the promise chains of fetch and the answer's reads run.
function settle(): Promise<void> {
  return new Promise((resolve) => setTimeout(resolve, 0))
}

async function isOpen(recording: Recording): Promise<boolean> {
  return (await Promise.race([recording.closed.then(() => 'closed'), settle().then(() => 'open')])) === 'open'
}

test('the first line of the answer names the replay prefix and when the recording started', async () => {
  // Arrange
  const recorder = new FakeRecorder()

  // Act
  const recording = await recorder.open('jwt')

  // Assert
  expect(recording.prefix).toEqual(['app', 'room', 'recordings', 'rec-1'])
  expect(recording.startedAtMs).toBe(1_700_000_000_000)
  expect(recorder.answers[0].url).toBe(PUBLISH_URL)
  expect(recorder.answers[0].init.method).toBe('POST')
  expect(recorder.answers[0].authorization).toBe('Bearer jwt')
  expect(recorder.openAnswers).toBe(1)
})

test('a refused publish rejects with the status and the error the recorder sent', async () => {
  // Arrange
  const recorder = new FakeRecorder()
  const { opening } = await recorder.start()

  // Act
  recorder.answers[0].refuse(409, 'only finished recordings can be published')
  const error = await opening.catch((error: unknown) => error)

  // Assert
  expect(error).toBeInstanceOf(RecorderError)
  expect((error as RecorderError).status).toBe(409)
  expect((error as RecorderError).message).toBe('only finished recordings can be published')
  expect(recorder.answers[0].authorization).toBeNull()
})

test('a description without the renewal period rejects and closes the answer', async () => {
  // Arrange
  const recorder = new FakeRecorder()
  const { opening } = await recorder.start()

  // Act
  recorder.answers[0].describe('{"namespace_prefix":"app/room/recordings/rec-1"}')
  const error = await opening.catch((error: unknown) => error)

  // Assert
  expect(error).toBeInstanceOf(Error)
  expect(recorder.answers[0].isClosed).toBe(true)
})

test('a renewal opens the next answer and closes the previous one a minute later', async () => {
  // Arrange
  const tokens = ['first', 'second']
  const recorder = new FakeRecorder()
  await recorder.open(async () => tokens.shift()!)

  // Act
  await recorder.advanceBy(RENEW_AFTER_MS)
  recorder.answers[1].describe()
  await recorder.advanceBy(OVERLAP_MS - 1)
  const openDuringOverlap = recorder.openAnswers
  await recorder.advanceBy(1)

  // Assert: the token function is asked again for the renewal
  expect(openDuringOverlap).toBe(2)
  expect(recorder.answers.map((answer) => answer.isClosed)).toEqual([true, false])
  expect(recorder.answers[1].authorization).toBe('Bearer second')
})

test('the next renewal is counted from the answer that replaced the previous one', async () => {
  // Arrange
  const recorder = new FakeRecorder()
  await recorder.open()
  await recorder.advanceBy(RENEW_AFTER_MS)
  recorder.answers[1].describe()

  // Act
  await recorder.advanceBy(RENEW_AFTER_MS)

  // Assert
  expect(recorder.answers).toHaveLength(3)
})

test('an answer that ends without the page closing it is replaced at once', async () => {
  // Arrange
  const recorder = new FakeRecorder()
  const recording = await recorder.open()

  // Act
  recorder.answers[0].end()
  await recorder.advanceBy(0)
  recorder.answers[1].describe()
  await recorder.advanceBy(0)

  // Assert
  expect(recorder.answers).toHaveLength(2)
  expect(recorder.answers[1].isClosed).toBe(false)
  expect(await isOpen(recording)).toBe(true)
})

test('a failed renewal keeps the current answer', async () => {
  // Arrange
  const recorder = new FakeRecorder()
  const recording = await recorder.open()
  await recorder.advanceBy(RENEW_AFTER_MS)

  // Act
  recorder.answers[1].refuse(503, 'too many recordings are being replayed')
  await recorder.advanceBy(0)

  // Assert
  expect(recorder.answers[0].isClosed).toBe(false)
  expect(await isOpen(recording)).toBe(true)
})

test('the recording closes with the error when an ended answer cannot be replaced', async () => {
  // Arrange
  const recorder = new FakeRecorder()
  const recording = await recorder.open()
  recorder.answers[0].end()
  await recorder.advanceBy(0)

  // Act
  recorder.answers[1].refuse(404, 'no such recording')

  // Assert
  const error = await recording.closed
  expect(error).toBeInstanceOf(RecorderError)
  expect((error as RecorderError).status).toBe(404)
  expect(recorder.pendingTimers).toBe(0)
})

test('close() closes the current answer, the one it replaced and the one being opened', async () => {
  // Arrange: the second answer asks for a renewal before the first one's overlap is over
  const recorder = new FakeRecorder()
  const recording = await recorder.open()
  await recorder.advanceBy(RENEW_AFTER_MS)
  recorder.answers[1].describe(
    '{"namespace_prefix":"app/room/recordings/rec-1","renew_after_seconds":30,"started_at_ms":1700000000000}'
  )
  await recorder.advanceBy(30_000)

  // Act
  recording.close()
  await recorder.advanceBy(0)

  // Assert: no timer is left to renew or to close the replaced answer later
  expect(await recording.closed).toBeUndefined()
  expect(recorder.answers).toHaveLength(3)
  expect(recorder.openAnswers).toBe(0)
  expect(recorder.pendingTimers).toBe(0)
})

test('the recording closes with its session', async () => {
  // Arrange
  const recorder = new FakeRecorder()
  const { promise: closed, resolve: closeSession } = Promise.withResolvers<CloseInfo>()
  const recording = await recorder.open(undefined, { closed })

  // Act
  closeSession({ code: undefined, reason: '' })

  // Assert
  expect(await recording.closed).toBeUndefined()
  expect(recorder.openAnswers).toBe(0)
})
