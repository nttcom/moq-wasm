export function parseAudioChannelCount(channelConfig?: string): number | undefined {
  const normalized = channelConfig?.trim().toLowerCase()
  if (!normalized) {
    return undefined
  }
  if (normalized === 'mono') {
    return 1
  }
  if (normalized === 'stereo') {
    return 2
  }
  const match = normalized.match(/^(\d+)ch$/)
  if (match) {
    const count = Number(match[1])
    return Number.isInteger(count) && count > 0 ? count : undefined
  }
  return undefined
}

export function formatAudioChannelConfig(channels: number): string {
  return channels === 1 ? 'mono' : channels === 2 ? 'stereo' : `${channels}ch`
}
