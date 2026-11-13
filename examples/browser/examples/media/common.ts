import { firstSearchParam } from '../../utils/relayPresets'

export function getMediaVideoCodecOverride(): string | undefined {
  return firstSearchParam(['videoCodec']) ?? undefined
}

export function getMediaVideoHardwareAcceleration(): HardwareAcceleration | undefined {
  const value = firstSearchParam(['videoEncoderHardwareAcceleration', 'videoHardwareAcceleration'])
  if (value === 'prefer-hardware' || value === 'prefer-software' || value === 'no-preference') {
    return value
  }
  return undefined
}
