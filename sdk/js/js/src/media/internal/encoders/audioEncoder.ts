import { louderAudioLevel, measureAudioLevel } from '../audioLevel'
import type { AudioLevel } from '../loc'
import { CaptureTimestamps } from './captureTimestamps'
import { encoderConfigForInput } from './audioInputFormat'
import { pcmFrames } from './pcmCapture'
import { EncoderSamplePosition } from './encoderSamplePosition'
import { workerLog } from '../../../core/log'

const TARGET = 'moq-wasm::media::audio_encoder'

let audioEncoder: AudioEncoder | undefined
let encoderConfig: AudioEncoderConfig = {
  codec: 'opus',
  sampleRate: 48000,
  numberOfChannels: 1,
  bitrate: 64_000
}
const captureTimestamps = new CaptureTimestamps()
const samplePosition = new EncoderSamplePosition()
/// An output frame spans several inputs, whose timestamps it does not keep, so
/// it carries the loudest level measured since the previous output.
let pendingAudioLevel: AudioLevel | undefined

function sendAudioChunkMessage(chunk: EncodedAudioChunk, metadata: EncodedAudioChunkMetadata | undefined) {
  const { init, captureTimestampMicros } = captureTimestamps.restamp(chunk)
  const audioLevel = pendingAudioLevel
  pendingAudioLevel = undefined
  self.postMessage({ type: 'chunk', chunk: new EncodedAudioChunk(init), metadata, captureTimestampMicros, audioLevel })
}

async function initializeAudioEncoder() {
  const init: AudioEncoderInit = {
    output: sendAudioChunkMessage,
    error: (e: any) => {
      workerLog('error', TARGET, 'encoder error', { error: e })
      self.postMessage({ type: 'encoderError', message: e.message })
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
  configure(encoder)
  workerLog('info', TARGET, 'encoder initialized', { config: encoderConfig })
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
    captureTimestamps.recordCapture(samplePosition.take(audio))
    pendingAudioLevel = louderAudioLevel(pendingAudioLevel, measureAudioLevel(audio))
    encoder.encode(audio)
    audio.close()
  }
}

function configure(encoder: AudioEncoder): void {
  encoder.configure(encoderConfig)
  samplePosition.reset()
}

/// An encoder error closes the encoder, so replace a closed one to keep the track sending.
async function encoderFor(audio: AudioData): Promise<AudioEncoder | undefined> {
  const inputConfig = encoderConfigForInput(encoderConfig, audio)
  if (inputConfig !== encoderConfig) {
    workerLog('info', TARGET, 'input format changed', {
      sampleRate: audio.sampleRate,
      numberOfChannels: audio.numberOfChannels
    })
    encoderConfig = inputConfig
    if (audioEncoder?.state === 'configured') {
      configure(audioEncoder)
    }
  }
  if (!audioEncoder || audioEncoder.state === 'closed') {
    if (audioEncoder) {
      workerLog('info', TARGET, 'encoder closed, re-initializing')
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
        configure(audioEncoder)
        workerLog('info', TARGET, 'encoder reconfigured', { config: encoderConfig })
      } catch (e) {
        self.postMessage({ type: 'configError', media: 'audio', reason: 'unsupported', config: encoderConfig })
        return
      }
    }
    return
  }
  // A page without MediaStreamTrackProcessor (Safari) sends the port of `capturePcm` instead.
  const audioReadableStream: ReadableStream<AudioData> | undefined =
    event.data.audioStream ?? (event.data.audioPort && pcmFrames(event.data.audioPort))
  if (!audioReadableStream) {
    workerLog('error', TARGET, 'no audio stream or port given')
    return
  }
  await startAudioEncode(audioReadableStream)
}
