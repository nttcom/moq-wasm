let audioEncoder: AudioEncoder | undefined
const AUDIO_ENCODER_CONFIG = {
  codec: 'opus',
  sampleRate: 48000, // Opusの推奨サンプルレート
  numberOfChannels: 2, // モノラル
  bitrate: 256000, // 64kbpsのビットレート
}

function sendAudioChunkMessage(chunk: EncodedAudioChunk, metadata: EncodedAudioChunkMetadata | undefined) {
  self.postMessage({ chunk, metadata })
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

let frameCounter = 0;

self.onmessage = async (event) => {
  const { type } = event.data;

  if (type === 'audioStream') {
    const audioReadableStream: ReadableStream<AudioData> = event.data.audioStream;
    if (!audioReadableStream) {
      console.error('MediaStreamTrack が渡されていません');
      return;
    }
    await startAudioEncode(audioReadableStream);
  } else if (type === 'audioData') {
    const { left, right, sampleRate } = event.data;

    // The data is already planar. We just need to combine the buffers.
    const combinedData = new Float32Array(left.length + right.length);
    combinedData.set(left, 0);
    combinedData.set(right, left.length);

    const duration = (left.length / sampleRate) * 1_000_000; // in microseconds
    const timestamp = frameCounter * duration;
    frameCounter++;

    const audioData = new AudioData({
      format: 'f32-planar',
      sampleRate: sampleRate,
      numberOfFrames: left.length,
      numberOfChannels: 2,
      timestamp: timestamp,
      data: combinedData,
    });

    if (!audioEncoder || audioEncoder.state === 'closed') {
        audioEncoder = await initializeAudioEncoder();
    }
    audioEncoder.encode(audioData);
    audioData.close();
  }
};
