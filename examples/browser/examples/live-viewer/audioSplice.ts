const MILLIS_PER_SECOND = 1_000
const CROSSFADE_MS = 5
const JOIN_SEARCH_MS = 10
/// The pitch periods of voices, 100–400 Hz: removing one whole period where
/// the waveform repeats shortens the sound without changing its pitch.
const MIN_PERIOD_MS = 2.5
const MAX_PERIOD_MS = 10
/// Below this the waveform does not repeat closely enough for the overlap to
/// go unheard, so nothing is removed.
const MIN_SIMILARITY = 0.8
const SILENCE_ENERGY = 1e-6

export type Channels = Float32Array<ArrayBuffer>[]

export function crossfadeSamples(sampleRate: number): number {
  return samplesFor(CROSSFADE_MS, sampleRate)
}

/// Joins the audio that follows dropped chunks onto the head of the first of
/// them, which is what the sound heard so far continues into, at the offset
/// where the two look most alike.
export function joinAfterSkip(skippedHead: Channels, next: Channels, sampleRate: number): Channels {
  const length = Math.min(crossfadeSamples(sampleRate), skippedHead[0].length)
  const latest = Math.min(samplesFor(JOIN_SEARCH_MS, sampleRate), next[0].length - length)
  if (latest < 0) {
    return next
  }
  const { offset } = mostSimilarOffset(mix(skippedHead).subarray(0, length), mix(next), 0, latest)
  return crossfade(skippedHead, next, offset, length)
}

/// Removes up to `maxSamples` from the start of a chunk, one repetition of its
/// waveform overlapped onto the next, or nothing when the waveform does not
/// repeat.
export function trimRepetition(
  channels: Channels,
  sampleRate: number,
  maxSamples: number
): { channels: Channels; removedSamples: number } | undefined {
  const length = crossfadeSamples(sampleRate)
  const earliest = samplesFor(MIN_PERIOD_MS, sampleRate)
  const latest = Math.min(samplesFor(MAX_PERIOD_MS, sampleRate), maxSamples, channels[0].length - length)
  if (latest < earliest) {
    return undefined
  }
  const mixed = mix(channels)
  const { offset, similarity } = mostSimilarOffset(mixed.subarray(0, length), mixed, earliest, latest)
  if (similarity < MIN_SIMILARITY) {
    return undefined
  }
  return { channels: crossfade(channels, channels, offset, length), removedSamples: offset }
}

/// Fades from the start of `from` into `to` at `toOffset`, then carries on
/// with the rest of `to`.
function crossfade(from: Channels, to: Channels, toOffset: number, length: number): Channels {
  return to.map((toChannel, channel) => {
    const out = new Float32Array(toChannel.length - toOffset)
    const fromChannel = from[channel] ?? from[0]
    for (let i = 0; i < length; i += 1) {
      const weight = 0.5 - 0.5 * Math.cos((Math.PI * (i + 0.5)) / length)
      out[i] = fromChannel[i] * (1 - weight) + toChannel[toOffset + i] * weight
    }
    out.set(toChannel.subarray(toOffset + length), length)
    return out
  })
}

/// Normalised cross-correlation of `reference` against `signal` at every
/// offset in `[earliest, latest]`. A silent stretch matches anywhere, so the
/// latest offset is taken and the most is removed.
function mostSimilarOffset(
  reference: Float32Array,
  signal: Float32Array,
  earliest: number,
  latest: number
): { offset: number; similarity: number } {
  const referenceEnergy = energy(reference, 0, reference.length)
  let best = { offset: latest, similarity: -1 }
  for (let offset = earliest; offset <= latest; offset += 1) {
    const signalEnergy = energy(signal, offset, reference.length)
    if (referenceEnergy < SILENCE_ENERGY && signalEnergy < SILENCE_ENERGY) {
      return { offset: latest, similarity: 1 }
    }
    let product = 0
    for (let i = 0; i < reference.length; i += 1) {
      product += reference[i] * signal[offset + i]
    }
    const similarity = product / Math.sqrt(referenceEnergy * signalEnergy || 1)
    if (similarity > best.similarity) {
      best = { offset, similarity }
    }
  }
  return best
}

function energy(signal: Float32Array, start: number, length: number): number {
  let sum = 0
  for (let i = start; i < start + length; i += 1) {
    sum += signal[i] * signal[i]
  }
  return sum
}

function mix(channels: Channels): Float32Array {
  if (channels.length === 1) {
    return channels[0]
  }
  const mixed = new Float32Array(channels[0].length)
  for (const channel of channels) {
    for (let i = 0; i < mixed.length; i += 1) {
      mixed[i] += channel[i]
    }
  }
  return mixed
}

function samplesFor(ms: number, sampleRate: number): number {
  return Math.round((ms * sampleRate) / MILLIS_PER_SECOND)
}
