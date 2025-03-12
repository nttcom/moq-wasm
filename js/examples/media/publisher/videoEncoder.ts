let videoEncoder: VideoEncoder | undefined
let keyframeInterval: number
const VIDEO_ENCODER_CONFIG = {
  codec: 'av01.0.04M.08',
  width: 640,
  height: 480,
  bitrate: 2_000_000, // 2 Mbps
  scalabilityMode: 'L1T3',
  framerate: 30
}

function sendVideoChunkMessage(chunk: EncodedVideoChunk, metadata: EncodedVideoChunkMetadata | undefined) {
  self.postMessage({ chunk, metadata })
}

async function initializeVideoEncoder() {
  const init: VideoEncoderInit = {
    output: sendVideoChunkMessage,
    error: (e: any) => {
      console.log(e.message)
    }
  }

  const encoder = new VideoEncoder(init)
  encoder.configure(VIDEO_ENCODER_CONFIG)
  return encoder
}

async function startVideoEncode(videoReadableStream: ReadableStream<VideoFrame>) {
  let frameCounter = 0
  if (!videoEncoder) {
    videoEncoder = await initializeVideoEncoder()
  }
  const videoReader = videoReadableStream.getReader()
  while (true) {
    const videoResult = await videoReader.read()
    if (videoResult.done) break
    const videoFrame = videoResult.value

    // Too many frames in flight, encoder is overwhelmed. let's drop this frame.
    if (videoEncoder.encodeQueueSize > 2) {
      console.error('videoEncoder.encodeQueueSize > 2', videoEncoder.encodeQueueSize)
      videoFrame.close()
    } else {
      const keyFrame = frameCounter % keyframeInterval == 0
      videoEncoder.encode(videoFrame, { keyFrame })
      frameCounter++
      videoFrame.close()
    }
  }
}

self.onmessage = async (event) => {
  if (event.data.type === 'keyframeInterval') {
    keyframeInterval = event.data.keyframeInterval
  } else if (event.data.type === 'videoStream') {
    const videoReadableStream: ReadableStream<VideoFrame> = event.data.videoStream
    if (!videoReadableStream) {
      console.error('MediaStreamTrack が渡されていません')
      return
    }
    await startVideoEncode(videoReadableStream)
  }
}
