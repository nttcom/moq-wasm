import type { MoqSession, PublishDone } from '../core/session'
import { sdkLog } from '../core/log'
import { botNamespace } from './bots'
import { callPage } from './internal/callPage'
import { isAtOrBefore } from './internal/location'
import { getErrorMessage } from './internal/status'
import { TranscriptTimeline, type TranscriptRecord } from './internal/transcriptTimeline'

export type { TranscriptRecord } from './internal/transcriptTimeline'

const TARGET = 'moq-wasm::media::transcript'
const TRANSCRIBER_BOT_TYPE = 'transcriber'
const TRANSCRIPT_TRACK_NAME = 'transcript'

export type TranscriptSubscription = {
  /// Resolves once the transcriber ends the transcript (PUBLISH_DONE), or to
  /// `undefined` once the session ends without one.
  readonly done: Promise<PublishDone | undefined>
  /// UNSUBSCRIBE. `onRecords` is not called after it.
  close(): Promise<void>
}

/// Follows the room's transcript, the event timeline the `transcriber` bot
/// publishes on the track `transcript` of `botNamespace(room, 'transcriber')`,
/// from the current group on (`MoqSession.subscribeCurrentGroup`). Calls
/// `onRecords` with every record of the timeline, in the order the transcriber
/// published them, each time they change; a record that came with history from
/// the joining FETCH is not `live`.
/// A malformed object or record is skipped. What `onRecords` throws is
/// reported with `reportError`, as an exception of an event listener is, and
/// the later records are still delivered. Rejects with the session's
/// `RequestError` when the relay rejects the SUBSCRIBE, as it does while the
/// transcriber is not in the room, so call it once `watchBroadcasts`'s
/// `onBotChange` or `botTypeOf` tells the transcriber arrived.
export async function subscribeTranscript(
  session: MoqSession,
  room: string[],
  onRecords: (records: TranscriptRecord[]) => void
): Promise<TranscriptSubscription> {
  const subscription = await session.subscribeCurrentGroup(
    botNamespace(room, TRANSCRIBER_BOT_TYPE),
    TRANSCRIPT_TRACK_NAME
  )
  const timeline = new TranscriptTimeline()
  let closed = false
  void (async () => {
    for await (const object of subscription) {
      if (closed) {
        return
      }
      const live = !isAtOrBefore(object, subscription.largest)
      if (timeline.add(object.groupId, object.objectId, object.objectPayload, live)) {
        const records = timeline.records()
        callPage(() => onRecords(records))
      }
    }
  })().catch((error: unknown) =>
    sdkLog(session, 'warn', TARGET, 'reading the transcript failed', { error: getErrorMessage(error) })
  )
  return {
    done: subscription.done,
    close: async () => {
      closed = true
      await subscription.close()
    }
  }
}
