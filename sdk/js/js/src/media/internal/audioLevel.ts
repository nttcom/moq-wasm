import type { AudioLevel } from './loc'

/// This level (-50 dBov) or louder counts as voice activity. Speech sits around -20 to -40
/// dBov; a quiet room is below -55.
const VOICE_ACTIVITY_MAX_LEVEL = 50
const QUIETEST_LEVEL = 127

/// The RFC 6464 level of samples in [-1, 1]: their RMS in -dBov, where a
/// full-scale square wave is 0.
export function audioLevelOfSamples(samples: Float32Array): AudioLevel {
  let sumOfSquares = 0
  for (const sample of samples) {
    sumOfSquares += sample * sample
  }
  const rms = samples.length > 0 ? Math.sqrt(sumOfSquares / samples.length) : 0
  const level = rms > 0 ? Math.min(QUIETEST_LEVEL, Math.round(-20 * Math.log10(rms))) : QUIETEST_LEVEL
  return { level: Math.max(0, level), voiceActivity: level <= VOICE_ACTIVITY_MAX_LEVEL }
}

/// The level of the first channel of `audio`.
export function measureAudioLevel(audio: AudioData): AudioLevel {
  const samples = new Float32Array(audio.numberOfFrames)
  audio.copyTo(samples, { planeIndex: 0, format: 'f32-planar' })
  return audioLevelOfSamples(samples)
}

export function louderAudioLevel(a: AudioLevel | undefined, b: AudioLevel): AudioLevel {
  return a === undefined || b.level < a.level ? b : a
}
