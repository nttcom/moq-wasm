let videoEncoder: VideoEncoder | undefined
let keyframeInterval: number

// H264 プロファイル
// Baseline: 42 Main: 4D High: 64
// H264 Level
// 4.0: 28 4.1: 29 4.2: 2A
// 5.0: 32 5.1: 33 5.2: 34
// M1 macではHWEncoderがL1T1しかサポートしていない

// const HW_VIDEO_ENCODER_CONFIG = {
//   codec: 'avc1.640028',
//   avc: {
//     format: 'annexb'
//   } as any,
//   hardwareAcceleration: 'prefer-hardware' as any,
//   width: 1920,
//   height: 1080,
//   bitrate: 5_000_000, //5 Mbps
//   scalabilityMode: 'L1T1',
//   framerate: 30,
//   latencyMode: 'realtime' as any
//   // latencyMode: 'quality' as any
// }

const SW_VIDEO_ENCODER_CONFIG = {
  codec: 'av01.0.08M.08',
  width: 1920,
  height: 1080,
  bitrate: 2_500_000, //10 Mbps
  // scalabilityMode: 'L1T3',
  scalabilityMode: 'L1T1',
  framerate: 30
}

// Mbps計測用の関数
function createBitrateLogger() {
  let bytesThisSecond = 0
  let lastLogTime = performance.now()
  return {
    addBytes(byteLength: number) {
      bytesThisSecond += byteLength
      const now = performance.now()
      if (now - lastLogTime >= 1000) {
        const mbps = (bytesThisSecond * 8) / 1_000_000
        console.log(`Encoded bitrate: ${mbps.toFixed(2)} Mbps`)
        bytesThisSecond = 0
        lastLogTime = now
      }
    }
  }
}

const bitrateLogger = createBitrateLogger()

function sendVideoChunkMessage(chunk: EncodedVideoChunk, metadata: EncodedVideoChunkMetadata | undefined) {
  bitrateLogger.addBytes(chunk.byteLength)
  self.postMessage({ chunk, metadata })
}

async function initializeVideoEncoder() {
  const init: VideoEncoderInit = {
    output: sendVideoChunkMessage,
    error: (e: any) => {
      console.log(e.message)
    }
  }
  console.log('isEncoderConfig Supported', await VideoEncoder.isConfigSupported(SW_VIDEO_ENCODER_CONFIG))
  const encoder = new VideoEncoder(init)
  encoder.configure(SW_VIDEO_ENCODER_CONFIG)
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
    if (videoEncoder.encodeQueueSize > 10) {
      console.error('videoEncoder.encodeQueueSize > 10', videoEncoder.encodeQueueSize)
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
