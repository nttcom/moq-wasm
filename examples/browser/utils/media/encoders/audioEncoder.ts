import { louderAudioLevel, measureAudioLevel } from '../audioLevel'
import type { AudioLevel } from '../loc'
import { createBitrateLogger } from '../bitrate'
import { CaptureTimestamps } from './captureTimestamps'
import { encoderConfigForInput } from './audioInputFormat'

let audioEncoder: AudioEncoder | undefined
let encoderConfig: AudioEncoderConfig = {
  codec: 'opus',
  sampleRate: 48000,
  numberOfChannels: 1,
  bitrate: 64_000
}
const captureTimestamps = new CaptureTimestamps()
/// An output frame spans several inputs, whose timestamps it does not keep, so
/// it carries the loudest level measured since the previous output.
let pendingAudioLevel: AudioLevel | undefined

const audioBitrateLogger = createBitrateLogger((kbps) => {
  self.postMessage({ type: 'bitrate', media: 'audio', kbps })
})

function sendAudioChunkMessage(chunk: EncodedAudioChunk, metadata: EncodedAudioChunkMetadata | undefined) {
  audioBitrateLogger.addBytes(chunk.byteLength)

  const { init, captureTimestampMicros } = captureTimestamps.restamp(chunk)
  const audioLevel = pendingAudioLevel
  pendingAudioLevel = undefined
  self.postMessage({ type: 'chunk', chunk: new EncodedAudioChunk(init), metadata, captureTimestampMicros, audioLevel })
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
  captureTimestamps.reset()
  const audioReader = audioReadableStream.getReader()
  while (true) {
    const audioResult = await audioReader.read()
    if (audioResult.done) break
    const audio = audioResult.value
    const encoder = await encoderFor(audio)
    if (!encoder) {
      audio.close()
      return
    }
    captureTimestamps.recordCapture(audio.timestamp)
    pendingAudioLevel = louderAudioLevel(pendingAudioLevel, measureAudioLevel(audio))
    encoder.encode(audio)
    audio.close()
  }
}

/// An encoder error closes the encoder, so replace a closed one to keep the track sending.
async function encoderFor(audio: AudioData): Promise<AudioEncoder | undefined> {
  const inputConfig = encoderConfigForInput(encoderConfig, audio)
  if (inputConfig !== encoderConfig) {
    console.info('[audioEncoder] input format changed', {
      sampleRate: audio.sampleRate,
      numberOfChannels: audio.numberOfChannels
    })
    encoderConfig = inputConfig
    if (audioEncoder?.state === 'configured') {
      audioEncoder.configure(encoderConfig)
    }
  }
  if (!audioEncoder || audioEncoder.state === 'closed') {
    if (audioEncoder) {
      console.info('[audioEncoder] re-initializing a closed encoder')
    }
    audioEncoder = await initializeAudioEncoder()
  }
  return audioEncoder
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
