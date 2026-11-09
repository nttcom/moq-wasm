export function isPcmAlawCodec(codec: string): boolean {
  const normalized = codec.trim().toLowerCase()
  return normalized === 'pcma' || normalized === 'pcm_alaw' || normalized === 'g711-alaw'
}

/// WebCodecs has no A-law decoder, so A-law frames are expanded to 16-bit PCM here.
export function alawToAudioData(
  payload: Uint8Array,
  sampleRate: number,
  channels: number,
  timestamp: number
): AudioData | undefined {
  const numberOfChannels = Math.max(1, channels)
  const numberOfFrames = Math.floor(payload.byteLength / numberOfChannels)
  if (numberOfFrames <= 0) {
    return undefined
  }
  const pcm = new Int16Array(numberOfFrames * numberOfChannels)
  for (let i = 0; i < pcm.length; i += 1) {
    pcm[i] = decodeAlawSample(payload[i] ?? 0)
  }
  return new AudioData({
    format: 's16',
    sampleRate: Math.max(1, sampleRate),
    numberOfFrames,
    numberOfChannels,
    timestamp,
    data: new Uint8Array(pcm.buffer)
  })
}

function decodeAlawSample(encoded: number): number {
  const sample = encoded ^ 0x55
  let value = (sample & 0x0f) << 4
  const segment = (sample & 0x70) >> 4
  switch (segment) {
    case 0:
      value += 8
      break
    case 1:
      value += 0x108
      break
    default:
      value += 0x108
      value <<= segment - 1
      break
  }
  return (sample & 0x80) !== 0 ? value : -value
}
