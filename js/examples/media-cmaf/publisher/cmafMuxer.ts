// eslint-disable-next-line @typescript-eslint/ban-ts-comment
// @ts-ignore -- mp4box v0.5.4 has no type declarations
import MP4Box, { ISOFile } from 'mp4box'

export type CmafChunkCallback = (
  type: 'init' | 'media',
  chunk: ArrayBuffer,
  isKeyframe: boolean
) => void | Promise<void>

/**
 * Muxes EncodedVideoChunks (AVCC format) into CMAF chunks (moof+mdat)
 * using mp4box.js.
 *
 * addSample() internally creates moof+mdat boxes in the ISOFile.
 * We extract the last two boxes after each addSample call and write
 * them out as a CMAF chunk.
 *
 * Usage:
 *   const muxer = new CmafVideoMuxer((type, chunk, isKeyframe) => { ... })
 *   muxer.addVideoChunk(chunk, metadata)
 */
export class CmafVideoMuxer {
  private mp4: any
  private trackId: number | null = null
  private initSegmentEmitted = false
  private onChunkCallback: CmafChunkCallback
  private timescale = 90_000
  private nextDts = 0

  constructor(onChunk: CmafChunkCallback) {
    this.onChunkCallback = onChunk
    this.mp4 = MP4Box.createFile()
  }

  addVideoChunk(chunk: EncodedVideoChunk, metadata: EncodedVideoChunkMetadata | undefined): void {
    // Wait for first keyframe with decoder config to create the track
    if (this.trackId === null) {
      if (chunk.type !== 'key') {
        console.debug('[CmafMuxer] waiting for first keyframe, skipping')
        return
      }

      const decoderConfig = (metadata as any)?.decoderConfig
      const description = decoderConfig?.description as ArrayBuffer | undefined
      if (!description) {
        console.warn('[CmafMuxer] keyframe without decoderConfig.description, skipping')
        return
      }

      this.trackId = this.mp4.addTrack({
        type: 'avc1',
        timescale: this.timescale,
        width: decoderConfig?.codedWidth ?? 1920,
        height: decoderConfig?.codedHeight ?? 1080,
        avcDecoderConfigRecord: description,
      })

      console.info('[CmafMuxer] track created', { trackId: this.trackId })
    }

    // Convert chunk data to Uint8Array
    const data = new Uint8Array(chunk.byteLength)
    chunk.copyTo(data)

    // Calculate duration in timescale units
    const duration = chunk.duration
      ? Math.round((chunk.duration / 1_000_000) * this.timescale)
      : Math.round(this.timescale / 30)

    const isKeyframe = chunk.type === 'key'

    // Track how many boxes exist before addSample
    const boxCountBefore = this.mp4.boxes.length

    this.mp4.addSample(this.trackId, data, {
      duration,
      dts: this.nextDts,
      cts: this.nextDts,
      is_sync: isKeyframe,
    })

    this.nextDts += duration

    // Emit init segment on first sample
    if (!this.initSegmentEmitted) {
      const initSegment = ISOFile.writeInitializationSegment(
        this.mp4.ftyp,
        this.mp4.moov,
        0
      )
      console.info('[CmafMuxer] init segment emitted', { byteLength: initSegment.byteLength })
      this.onChunkCallback('init', initSegment, true)
      this.initSegmentEmitted = true
    }

    // addSample appends moof + mdat boxes - write them out as a CMAF chunk
    const newBoxes = this.mp4.boxes.slice(boxCountBefore)
    if (newBoxes.length >= 2) {
      const stream = new MP4Box.DataStream()
      stream.endianness = MP4Box.DataStream.BIG_ENDIAN
      for (const box of newBoxes) {
        box.write(stream)
      }
      const buffer = stream.buffer
      console.debug('[CmafMuxer] media chunk', {
        isKeyframe,
        byteLength: buffer.byteLength,
        boxes: newBoxes.map((b: any) => b.type),
      })
      this.onChunkCallback('media', buffer, isKeyframe)
    } else {
      console.warn('[CmafMuxer] expected moof+mdat boxes after addSample, got', newBoxes.length)
    }
  }
}
