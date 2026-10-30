/// One message of `pcmCaptureWorklet.js`: `data` holds each channel's frames in turn.
export type PcmBatch = {
  timestampMicros: number
  sampleRate: number
  numberOfChannels: number
  numberOfFrames: number
  data: Float32Array<ArrayBuffer>
}

export type PcmCapture = {
  /// Carries the `PcmBatch` messages; transfer it to the encoder worker.
  port: MessagePort
  /// Stops capturing; the track keeps running.
  close: () => void
}

/// The audio of `track` as 10 ms PCM batches, for a page without
/// `MediaStreamTrackProcessor` (Safari), which reads audio only through Web Audio.
export async function capturePcm(track: MediaStreamTrack): Promise<PcmCapture> {
  const context = new AudioContext()
  try {
    await context.audioWorklet.addModule(new URL('./pcmCaptureWorklet.js', import.meta.url))
    const source = context.createMediaStreamSource(new MediaStream([track]))
    const node = new AudioWorkletNode(context, 'pcm-capture')
    const { port1, port2 } = new MessageChannel()
    node.port.postMessage(port1, [port1])
    source.connect(node)
    // Connect to the destination so the graph pulls the node; it writes no
    // output, so this plays silence.
    node.connect(context.destination)
    void context.resume()
    return {
      port: port2,
      close: () => {
        source.disconnect()
        void context.close()
      }
    }
  } catch (error) {
    void context.close()
    throw error
  }
}

/// The batches `capturePcm` posts on `port`, as the frames
/// `MediaStreamTrackProcessor` reads from an audio track.
export function pcmFrames(port: MessagePort): ReadableStream<AudioData> {
  return new ReadableStream({
    start(controller) {
      port.onmessage = ({ data }: MessageEvent<PcmBatch>) => {
        controller.enqueue(
          new AudioData({
            format: 'f32-planar',
            sampleRate: data.sampleRate,
            numberOfChannels: data.numberOfChannels,
            numberOfFrames: data.numberOfFrames,
            timestamp: data.timestampMicros,
            data: data.data
          })
        )
      }
    }
  })
}
