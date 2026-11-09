/// Chrome's AudioEncoder closes with an EncodingError when the input's sample
/// rate or channel count differs from its config. A capture device can switch
/// format mid-call, e.g. a Bluetooth headset changing profile. Returns the same
/// `config` object when the input matches, so callers detect a change with `!==`.
export function encoderConfigForInput(
  config: AudioEncoderConfig,
  input: Pick<AudioData, 'sampleRate' | 'numberOfChannels'>
): AudioEncoderConfig {
  if (config.sampleRate === input.sampleRate && config.numberOfChannels === input.numberOfChannels) {
    return config
  }
  return { ...config, sampleRate: input.sampleRate, numberOfChannels: input.numberOfChannels }
}
