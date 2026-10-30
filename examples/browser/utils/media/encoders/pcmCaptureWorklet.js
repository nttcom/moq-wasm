// An AudioWorklet module the build copies as is (it is a `new URL` asset, not a
// bundled worker), so it stays plain JS and self-contained.

/// The capture inputs of Chrome's MediaStreamTrackProcessor are 10 ms apart,
/// which `CaptureTimestamps` relies on to match an encoder output to its input.
const BATCH_SECONDS = 0.01

/// Posts its input as planar 10 ms batches on the port the page sends it,
/// stamped with the context time of their first frame.
class PcmCapture extends AudioWorkletProcessor {
  constructor() {
    super()
    this.batchFrames = Math.round(sampleRate * BATCH_SECONDS)
    this.channels = []
    this.filled = 0
    this.firstFrame = 0
    this.port.onmessage = (event) => {
      this.output = event.data
    }
  }

  process(inputs) {
    const input = inputs[0]
    if (!this.output || input.length === 0) {
      return true
    }
    if (this.channels.length !== input.length) {
      this.channels = input.map(() => new Float32Array(this.batchFrames))
      this.filled = 0
    }
    let offset = 0
    while (offset < input[0].length) {
      if (this.filled === 0) {
        this.firstFrame = currentFrame + offset
      }
      const count = Math.min(input[0].length - offset, this.batchFrames - this.filled)
      input.forEach((samples, channel) => {
        this.channels[channel].set(samples.subarray(offset, offset + count), this.filled)
      })
      this.filled += count
      offset += count
      if (this.filled === this.batchFrames) {
        this.post()
      }
    }
    return true
  }

  post() {
    const data = new Float32Array(this.batchFrames * this.channels.length)
    this.channels.forEach((samples, channel) => data.set(samples, channel * this.batchFrames))
    this.output.postMessage(
      {
        timestampMicros: Math.round((this.firstFrame / sampleRate) * 1_000_000),
        sampleRate,
        numberOfChannels: this.channels.length,
        numberOfFrames: this.batchFrames,
        data
      },
      [data.buffer]
    )
    this.filled = 0
  }
}

registerProcessor('pcm-capture', PcmCapture)
