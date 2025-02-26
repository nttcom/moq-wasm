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
  const config = {
    codec: 'vp8',
    width: 640,
    height: 480,
    bitrate: 2_000_000, // 2 Mbps
    framerate: 30
  }
  const encoder = new VideoEncoder(init)
  encoder.configure(config)
  return encoder
}

async function startEncode(videoReadableStream: ReadableStream<VideoFrame>) {
  let frameCounter = 0
  const videoEncoder = await initializeVideoEncoder()
  const videoReader = videoReadableStream.getReader()
  while (true) {
    const videoResult = await videoReader.read()
    if (videoResult.done) break
    const videoFrame = videoResult.value

    // Too many frames in flight, encoder is overwhelmed. let's drop this frame.
    if (videoEncoder.encodeQueueSize > 2) {
      videoFrame.close()
    } else {
      frameCounter++
      const keyFrame = frameCounter % 150 == 0
      videoEncoder.encode(videoFrame, { keyFrame })
      videoFrame.close()
    }
  }
}

self.onmessage = async (event) => {
  const videoReadableStream: ReadableStream<VideoFrame> = event.data.videoStream
  if (!videoReadableStream) {
    console.error('MediaStreamTrack が渡されていません')
    return
  }
  await startEncode(videoReadableStream)
}
