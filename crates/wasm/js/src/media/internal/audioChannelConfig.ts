const NAMED_CHANNEL_COUNTS = new Map([
  ['mono', 1],
  ['stereo', 2]
])

export function parseAudioChannelCount(channelConfig?: string): number | undefined {
  if (channelConfig === undefined) {
    return undefined
  }
  return NAMED_CHANNEL_COUNTS.get(channelConfig) ?? (Number(channelConfig.match(/^(\d+)ch$/)?.[1]) || undefined)
}
