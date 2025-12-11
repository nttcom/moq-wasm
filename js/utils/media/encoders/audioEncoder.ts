import { createBitrateLogger } from '../bitrate'

let audioEncoder: AudioEncoder | undefined
let timestampOffset: number | null = null
let encoderConfig: AudioEncoderConfig = {
  codec: 'opus',
  sampleRate: 48000,
  numberOfChannels: 1,
  bitrate: 64_000
}

const audioBitrateLogger = createBitrateLogger((kbps) => {
  self.postMessage({ type: 'bitrate', media: 'audio', kbps })
})

function sendAudioChunkMessage(chunk: EncodedAudioChunk, metadata: EncodedAudioChunkMetadata | undefined) {
  audioBitrateLogger.addBytes(chunk.byteLength)
  console.debug('sendAudioChunkMessage', chunk, metadata)

  // 最初のチャンクでtimestampのoffsetを保存
  if (timestampOffset === null) {
    timestampOffset = chunk.timestamp
    console.info('[audioEncoder] Set timestamp offset:', timestampOffset)
  }

  // timestampを0起点に調整したチャンクを作成
  const adjustedTimestamp = chunk.timestamp - timestampOffset
  const buffer = new ArrayBuffer(chunk.byteLength)
  chunk.copyTo(buffer)
  const adjustedChunk = new EncodedAudioChunk({
    type: chunk.type,
    timestamp: adjustedTimestamp,
    duration: chunk.duration ?? undefined,
    data: buffer
  })

  self.postMessage({ type: 'chunk', chunk: adjustedChunk, metadata })
}

async function initializeAudioEncoder() {
  const init: AudioEncoderInit = {
    output: sendAudioChunkMessage,
    error: (e: any) => {
      console.log(e.message)
    }
  }

  const encoder = new AudioEncoder(init)
  try {
    const supported = await AudioEncoder.isConfigSupported(encoderConfig)
    if (!supported.supported) {
      self.postMessage({ type: 'configError', media: 'audio', reason: 'unsupported', config: encoderConfig })
      return undefined
    }
  } catch (e) {
    self.postMessage({ type: 'configError', media: 'audio', reason: 'unsupported', config: encoderConfig })
    return undefined
  }
  encoder.configure(encoderConfig)
  console.info('[audioEncoder] initialized', encoderConfig)
  return encoder
}

async function startAudioEncode(audioReadableStream: ReadableStream<AudioData>) {
  // 新しいストリーム開始時にtimestampのoffsetをリセット
  timestampOffset = null
  if (!audioEncoder) {
    audioEncoder = await initializeAudioEncoder()
  }
  if (!audioEncoder) {
    return
  }
  const audioReader = audioReadableStream.getReader()
  while (true) {
    const audioResult = await audioReader.read()
    if (audioResult.done) break
    const audio = audioResult.value
    audioEncoder.encode(audio)
    audio.close()
  }
}

self.onmessage = async (event) => {
  if (event.data.type === 'config') {
    const cfg = event.data.config as Partial<AudioEncoderConfig>
    encoderConfig = {
      ...encoderConfig,
      ...cfg
    }
    if (audioEncoder && audioEncoder.state !== 'closed') {
      try {
        const supported = await AudioEncoder.isConfigSupported(encoderConfig)
        if (!supported.supported) {
          self.postMessage({ type: 'configError', media: 'audio', reason: 'unsupported', config: encoderConfig })
          return
        }
        audioEncoder.configure(encoderConfig)
        console.info('[audioEncoder] reconfigured', encoderConfig)
      } catch (e) {
        self.postMessage({ type: 'configError', media: 'audio', reason: 'unsupported', config: encoderConfig })
        return
      }
    }
    return
  }
  const audioReadableStream: ReadableStream<AudioData> = event.data.audioStream
  if (!audioReadableStream) {
    console.error('MediaStreamTrack が渡されていません')
    return
  }
  await startAudioEncode(audioReadableStream)
}
