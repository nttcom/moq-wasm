import type { MediaCatalogTrack } from '../../media/catalog'
import { monotonicUnixMicros } from '../../../utils/media/clock'
import { postAudioCatalogToWorker, postVideoCatalogToWorker } from '../../../utils/media/decoderCatalog'
import { postSubgroupObjectToWorker } from '../../../utils/media/decoderWorker'
import type { BufferPolicy } from '../jitterBuffer'
import { LivePlayout } from '../livePlayout'
import { type LivePictureKind, type LivePictureSink, createLivePictureSink } from '../livePictureSink'
import { ANNEX_B_FORMAT } from './streamConventions'
import type { MediaKind, SubgroupObject } from './trackContext'

const PRESENTATION_MARGIN_MS = 200

export type DecodedFrameIds = { groupId: bigint; objectId: bigint }

export type LocLiveCallbacks = {
  onPresented(picture: HTMLElement): void
  onFrameShown(ids: DecodedFrameIds | undefined, captureMicros: number): void
}

export class LocLive {
  readonly playout: LivePlayout
  readonly picture: LivePictureSink
  private readonly videoWorker = new Worker(new URL('../../../utils/media/decoders/videoDecoder.ts', import.meta.url), {
    type: 'module'
  })
  private readonly audioWorker = new Worker(new URL('../../../utils/media/decoders/audioDecoder.ts', import.meta.url), {
    type: 'module'
  })
  private readonly decodedFrameIds = new Map<number, DecodedFrameIds>()
  receivedKbps = 0
  /// Live LOC frames carry their capture timestamp, so the moment one is shown
  /// says how far the viewer runs behind the publisher on the same wall clock.
  viewerDelayMs: number | undefined
  frameSize: { width: number; height: number } | undefined

  constructor(
    video: HTMLVideoElement,
    canvas: HTMLCanvasElement,
    livePicture: LivePictureKind | undefined,
    private readonly callbacks: LocLiveCallbacks
  ) {
    this.picture = createLivePictureSink(video, canvas, (picture) => callbacks.onPresented(picture), livePicture)
    this.playout = new LivePlayout(
      (frame) => this.show(frame),
      (origin) =>
        this.videoWorker.postMessage({
          type: 'timeline',
          captureMicros: origin?.captureMicros,
          dueAtUnixMs: origin && performance.timeOrigin + origin.atMs
        })
    )
    this.applyDecoderConfig()
    this.videoWorker.onmessage = (event) => {
      if (event.data.type === 'bitrate') {
        this.receivedKbps = event.data.kbps ?? this.receivedKbps
        return
      }
      if (event.data.type === 'frame') {
        const frame = event.data.frame as VideoFrame
        this.decodedFrameIds.set(frame.timestamp, { groupId: event.data.groupId, objectId: event.data.objectId })
        this.playout.presentVideo(frame)
      }
    }
    this.audioWorker.onmessage = (event) => {
      if (event.data.type === 'audioData') {
        this.playout.playAudio(
          event.data.audioData as AudioData,
          event.data.captureTimestampMicros as number | undefined
        )
      }
    }
  }

  configureTrack(kind: MediaKind, track: MediaCatalogTrack): void {
    if (kind === 'video') {
      postVideoCatalogToWorker(this.videoWorker, {
        codec: track.codec,
        initData: track.initData,
        avcFormat: track.initData ? undefined : ANNEX_B_FORMAT
      })
      return
    }
    postAudioCatalogToWorker(this.audioWorker, track)
  }

  push(kind: MediaKind, groupId: bigint, object: SubgroupObject): void {
    postSubgroupObjectToWorker(kind === 'video' ? this.videoWorker : this.audioWorker, groupId, object)
  }

  setBufferPolicy(policy: BufferPolicy): void {
    this.playout.setBufferPolicy(policy)
    this.applyDecoderConfig()
  }

  reset(): void {
    this.viewerDelayMs = undefined
    this.picture.detach()
    this.decodedFrameIds.clear()
    this.playout.reset()
  }

  private show(frame: VideoFrame): void {
    this.viewerDelayMs = frame.timestamp ? (monotonicUnixMicros() - frame.timestamp) / 1_000 : undefined
    this.frameSize = { width: frame.displayWidth, height: frame.displayHeight }
    const ids = this.decodedFrameIds.get(frame.timestamp)
    this.decodedFrameIds.delete(frame.timestamp)
    this.callbacks.onFrameShown(ids, frame.timestamp)
    this.picture.present(frame)
  }

  /// The live playout paces decoded samples on one clock so that audio and
  /// video stay together. All but the last `PRESENTATION_MARGIN_MS` of the
  /// minimum buffer is spent before decoding, in the video worker's jitter
  /// buffer, so objects are decoded in order however they arrived and only a
  /// few decoded frames are ever held.
  private applyDecoderConfig(): void {
    const holdMs = Math.max(0, this.playout.bufferPolicy().minimumMs - PRESENTATION_MARGIN_MS)
    this.videoWorker.postMessage({
      type: 'config',
      config: {
        telemetryEnabled: true,
        bypassJitterBuffer: holdMs === 0,
        holdMs,
        releaseMarginMs: PRESENTATION_MARGIN_MS,
        pacing: { preset: 'disabled' }
      }
    })
    this.audioWorker.postMessage({ type: 'config', config: { telemetryEnabled: true, bypassJitterBuffer: true } })
  }
}
