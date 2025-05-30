let audioEncoder: AudioEncoder | undefined
const AUDIO_ENCODER_CONFIG = {
  codec: 'opus',
  sampleRate: 48000, // Opusの推奨サンプルレート
  numberOfChannels: 1, // モノラル
  bitrate: 64000 // 64kbpsのビットレート
}

function sendAudioChunkMessage(chunk: EncodedAudioChunk, metadata: EncodedAudioChunkMetadata | undefined) {
  // self.postMessage({ chunk, metadata })
}

async function initializeAudioEncoder() {
  const init: AudioEncoderInit = {
    output: sendAudioChunkMessage,
    error: (e: any) => {
      console.log(e.message)
    }
  }

  const encoder = new AudioEncoder(init)
  encoder.configure(AUDIO_ENCODER_CONFIG)
  return encoder
}

async function startAudioEncode(audioReadableStream: ReadableStream<AudioData>) {
  if (!audioEncoder) {
    audioEncoder = await initializeAudioEncoder()
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
  const audioReadableStream: ReadableStream<AudioData> = event.data.audioStream
  if (!audioReadableStream) {
    console.error('MediaStreamTrack が渡されていません')
    return
  }
  await startAudioEncode(audioReadableStream)
}
