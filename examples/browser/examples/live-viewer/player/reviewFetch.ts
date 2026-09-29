import { RequestError, RequestErrorCode } from '@moqt/moqtClient'
import { getErrorMessage } from '../../media/common'
import { type ReviewFrame, toReviewFrame } from '../rewind'
import type { TrackContext } from './trackContext'

const FETCH_DEADLINE_MS = 8_000

/// The relay evicts the oldest groups first, so a FETCH failing with one of
/// these for a range it once cached means the range has aged out.
const EVICTED_RANGE_CODES: bigint[] = [
  RequestErrorCode.InternalError,
  RequestErrorCode.Timeout,
  RequestErrorCode.InvalidRange,
  RequestErrorCode.NoObjects,
  RequestErrorCode.UnknownStatusInRange
]

/// draft-ietf-moq-transport-14 §10.4.3 INTERNAL_ERROR: the relay resets a
/// FETCH stream with it when the range it is serving loses cached objects.
const STREAM_RESET_INTERNAL_ERROR = 0x0n

export type FetchFailure = {
  evicted: boolean
  description: string
}

type FetchStreamEnd = { kind: 'fin' } | { kind: 'reset'; code: bigint | undefined } | { kind: 'deadline' }

export function isFetchFailure(result: ReviewFrame[] | FetchFailure): result is FetchFailure {
  return !Array.isArray(result)
}

export async function fetchFrames(
  context: TrackContext,
  trackName: string,
  start: bigint,
  end: bigint,
  isCurrent: () => boolean
): Promise<ReviewFrame[] | FetchFailure | undefined> {
  const frames: ReviewFrame[] = []
  let requestId: bigint | undefined
  let endStream: (end: FetchStreamEnd) => void = () => {}
  const streamEnd = new Promise<FetchStreamEnd>((resolve) => {
    endStream = resolve
  })
  try {
    ;({ requestId } = await context.client.fetch(context.namespace, trackName, start, 0n, end, 0n, {
      onObject: (message) => {
        const frame = toReviewFrame(message)
        context.observer.fetchObject(
          message.requestId,
          trackName,
          message.groupId,
          message.objectId,
          message.objectPayload.byteLength,
          frame?.captureMicros
        )
        if (!isCurrent()) {
          return
        }
        if (frame) {
          frame.requestId = message.requestId
          frames.push(frame)
        }
      },
      onStreamEnd: (message) =>
        endStream(message.isReset ? { kind: 'reset', code: message.resetErrorCode } : { kind: 'fin' })
    }))
  } catch (error) {
    if (!isCurrent()) {
      return undefined
    }
    return {
      evicted: error instanceof RequestError && EVICTED_RANGE_CODES.includes(error.errorCode),
      description: getErrorMessage(error)
    }
  }

  const outcome = await waitForFetchStreamEnd(streamEnd)
  if (requestId !== undefined) {
    context.observer.fetchFinished(requestId)
  }
  if (!isCurrent()) {
    return undefined
  }
  if (outcome.kind === 'reset') {
    return {
      evicted: outcome.code === STREAM_RESET_INTERNAL_ERROR,
      description: `fetch stream reset (code ${outcome.code ?? 'unknown'})`
    }
  }
  if (outcome.kind === 'deadline') {
    context.log(
      'warn',
      `fetch ${trackName}: no stream end within ${FETCH_DEADLINE_MS} ms, playing ${frames.length} objects`
    )
  }
  return frames
}

/// The relay FINs the fetch stream once every object up to the FETCH_OK End
/// Location is written; the deadline only guards against a stream that never
/// ends.
async function waitForFetchStreamEnd(streamEnd: Promise<FetchStreamEnd>): Promise<FetchStreamEnd> {
  let timer: ReturnType<typeof setTimeout> | undefined
  const deadline = new Promise<FetchStreamEnd>((resolve) => {
    timer = setTimeout(() => resolve({ kind: 'deadline' }), FETCH_DEADLINE_MS)
  })
  try {
    return await Promise.race([streamEnd, deadline])
  } finally {
    clearTimeout(timer)
  }
}
