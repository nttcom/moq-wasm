import { firstSearchParam } from '../../utils/relayPresets'

export type MediaVideoHardwareAcceleration = 'prefer-hardware' | 'prefer-software' | 'no-preference'

export type MediaVideoEncodingOverrides = {
  codec?: string
  hardwareAcceleration?: MediaVideoHardwareAcceleration
}

export function getMediaVideoEncodingOverrides(): MediaVideoEncodingOverrides {
  const codec = firstSearchParam(['videoCodec'])
  const hardwareAcceleration = parseMediaVideoHardwareAcceleration(
    firstSearchParam(['videoEncoderHardwareAcceleration', 'videoHardwareAcceleration'])
  )
  return {
    codec: codec ?? undefined,
    hardwareAcceleration
  }
}

function parseMediaVideoHardwareAcceleration(value: string | null): MediaVideoHardwareAcceleration | undefined {
  if (value === 'prefer-hardware' || value === 'prefer-software' || value === 'no-preference') {
    return value
  }
  return undefined
}
