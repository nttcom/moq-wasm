import { RequestError, RequestErrorCode } from '../../core/requestError'
import { type ReviewFrame, toReviewFrame } from './rewind'
import { type TrackContext, sessionOf, streamId } from './trackContext'
import { getErrorMessage } from '../internal/status'

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

export type FetchFailure = {
  evicted: boolean
  description: string
}

type FetchStreamEnd = { kind: 'fin' } | { kind: 'reset'; description: string } | { kind: 'deadline' }

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
  const requestId = streamId()
  let response
  try {
    response = await sessionOf(context).fetch(
      context.namespace,
      trackName,
      { groupId: start, objectId: 0n },
      { groupId: end, objectId: 0n }
    )
  } catch (error) {
    if (!isCurrent()) {
      return undefined
    }
    return {
      evicted: error instanceof RequestError && EVICTED_RANGE_CODES.includes(error.errorCode),
      description: getErrorMessage(error)
    }
  }

  const streamEnd = (async (): Promise<FetchStreamEnd> => {
    try {
      for await (const object of response) {
        const frame = toReviewFrame(object)
        context.observer.fetchObject(
          requestId,
          trackName,
          object.groupId,
          object.objectId,
          object.objectPayloadLength,
          frame?.captureMicros
        )
        if (frame && isCurrent()) {
          frame.requestId = requestId
          frames.push(frame)
        }
      }
      return { kind: 'fin' }
    } catch (error) {
      return { kind: 'reset', description: getErrorMessage(error) }
    }
  })()
  const outcome = await waitForFetchStreamEnd(streamEnd)
  context.observer.fetchFinished(requestId)
  if (!isCurrent()) {
    return undefined
  }
  // Any reset is read as eviction: the relay resets a FETCH stream with
  // INTERNAL_ERROR (draft-ietf-moq-transport-14 §10.4.3) when the range it is
  // serving loses cached objects.
  if (outcome.kind === 'reset') {
    return { evicted: true, description: `fetch stream reset: ${outcome.description}` }
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
