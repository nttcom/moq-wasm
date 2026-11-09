import { RequestError, RequestErrorCode } from '../../core/requestError'
import type { MoqSession, PublishDone } from '../../core/session'
import { forwardSdkLog, sdkLog } from '../../core/log'
import { CATALOG_TRACK_NAME } from '../catalog'
import { getErrorMessage } from '../internal/status'
import { type DeliveryObserver, UNOBSERVED_DELIVERIES } from './deliveryObserver'
import { TextTracks } from './textTrack'
import type { LogLevel, TrackContext } from './trackContext'

const PUBLISHER_RETRY_MS = 2_000
const LOG_TARGET = 'moq-wasm::media::broadcast_catalog'

export type BroadcastCatalogOptions = {
  deliveryObserver?: DeliveryObserver
  /// Receives the catalog's logs instead of the console.
  onLog?: (level: LogLevel, message: string) => void
}

export type BroadcastCatalogListener = {
  /// The text of the newest catalog; a listener added after one arrived gets it at once.
  onCatalog(text: string): void
  /// Nobody publishes the namespace: the relay answered the catalog SUBSCRIBE
  /// with TRACK_DOES_NOT_EXIST, or the publisher ended the catalog with
  /// PUBLISH_DONE (`done`). The catalog SUBSCRIBE is retried until the
  /// publisher returns. A listener added meanwhile is told at once.
  onPublisherAbsent(done: PublishDone | undefined): void
}

type Latest = { kind: 'catalog'; text: string } | { kind: 'absent'; done: PublishDone | undefined }

/// The catalog of one broadcast, subscribed once for every reader on the
/// session: draft-ietf-moq-transport-14 §9.7 forbids a second subscription to
/// the same track on one session, so `LivePlayer`s that play sources of one
/// broadcast, and the page itself, share one.
///
/// A SUBSCRIBE delivers objects published after the largest one and the bridge
/// publishes the catalog once per upstream subscription, so a viewer joining a
/// subscription the relay already holds would never see it; the group the
/// largest object is in is read through a joining FETCH as well. The relay
/// keeps the catalog of a publisher that has since been replaced, so the
/// catalog of the newest group wins.
export class BroadcastCatalog {
  private readonly context: TrackContext
  private readonly textTracks: TextTracks
  private readonly listeners = new Set<BroadcastCatalogListener>()
  private latest: Latest | undefined
  private newestGroupId: bigint | undefined
  private starting: Promise<void> | undefined
  /// Incremented by every start and close; a `waitForPublisher` loop from an
  /// earlier start sees the change and returns.
  private generation = 0

  constructor(
    readonly session: MoqSession,
    readonly namespace: string[],
    options: BroadcastCatalogOptions = {}
  ) {
    this.context = {
      session,
      namespace,
      observer: options.deliveryObserver ?? UNOBSERVED_DELIVERIES,
      log: (level, message) => {
        if (options.onLog) {
          forwardSdkLog(level, LOG_TARGET, message)
          options.onLog(level, message)
        } else {
          sdkLog(level, LOG_TARGET, message)
        }
      }
    }
    this.textTracks = new TextTracks(this.context)
  }

  /// Resolves once the catalog is subscribed or, when nobody publishes the
  /// namespace yet, once it has started waiting for the publisher; rejects
  /// when the relay refuses the SUBSCRIBE otherwise. A later call returns the
  /// first call's promise until `close`.
  start(): Promise<void> {
    this.starting ??= this.run().catch((error: unknown) => {
      this.starting = undefined
      throw error
    })
    return this.starting
  }

  /// Returns the function that removes the listener.
  listen(listener: BroadcastCatalogListener): () => void {
    this.listeners.add(listener)
    if (this.latest?.kind === 'catalog') {
      listener.onCatalog(this.latest.text)
    } else if (this.latest?.kind === 'absent') {
      listener.onPublisherAbsent(this.latest.done)
    }
    return () => this.listeners.delete(listener)
  }

  /// Unsubscribes the catalog; `start` subscribes it again.
  async close(): Promise<void> {
    this.starting = undefined
    this.generation += 1
    this.newestGroupId = undefined
    this.latest = undefined
    await this.textTracks.unsubscribeAll()
  }

  private async run(): Promise<void> {
    this.generation += 1
    const generation = this.generation
    if (!(await this.subscribe(generation))) {
      void this.waitForPublisher(generation)
    }
  }

  /// Returns false when nobody publishes the namespace.
  private async subscribe(generation: number): Promise<boolean> {
    const onText = (text: string, groupId: bigint) => {
      if (this.newestGroupId !== undefined && groupId < this.newestGroupId) {
        return
      }
      this.newestGroupId = groupId
      this.update({ kind: 'catalog', text })
    }
    try {
      await this.textTracks.subscribe(CATALOG_TRACK_NAME, onText, {
        currentGroup: true,
        onEnded: (done) => this.publisherLeft(generation, done)
      })
    } catch (error) {
      if (!(error instanceof RequestError && error.errorCode === RequestErrorCode.TrackDoesNotExist)) {
        throw error
      }
      // A publisher that ended the catalog stays reported with its PUBLISH_DONE.
      if (generation === this.generation && this.latest?.kind !== 'absent') {
        this.update({ kind: 'absent', done: undefined })
      }
      return false
    }
    // A close while the SUBSCRIBE was in flight found nothing to unsubscribe.
    if (generation !== this.generation) {
      await this.textTracks.unsubscribeAll()
    }
    return true
  }

  private publisherLeft(generation: number, done: PublishDone): void {
    if (generation !== this.generation) {
      return
    }
    this.newestGroupId = undefined
    this.update({ kind: 'absent', done })
    void this.waitForPublisher(generation)
  }

  private async waitForPublisher(generation: number): Promise<void> {
    for (;;) {
      await new Promise((resolve) => setTimeout(resolve, PUBLISHER_RETRY_MS))
      if (generation !== this.generation) {
        return
      }
      try {
        if (await this.subscribe(generation)) {
          return
        }
      } catch (error) {
        this.context.log('error', `waiting for the publisher: ${getErrorMessage(error)}`)
        return
      }
    }
  }

  private update(latest: Latest): void {
    this.latest = latest
    for (const listener of this.listeners) {
      if (latest.kind === 'catalog') {
        listener.onCatalog(latest.text)
      } else {
        listener.onPublisherAbsent(latest.done)
      }
    }
  }
}
