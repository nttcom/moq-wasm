export type MediaVideoHardwareAcceleration = 'prefer-hardware' | 'prefer-software' | 'no-preference'

export type MediaVideoEncodingOverrides = {
  codec?: string
  hardwareAcceleration?: MediaVideoHardwareAcceleration
}

export function getMediaVideoEncodingOverrides(): MediaVideoEncodingOverrides {
  const params = new URLSearchParams(window.location.search)
  const codec = firstNonEmptyValue(params, ['videoCodec'])
  const hardwareAcceleration = parseMediaVideoHardwareAcceleration(
    firstNonEmptyValue(params, ['videoEncoderHardwareAcceleration', 'videoHardwareAcceleration'])
  )
  return {
    codec: codec ?? undefined,
    hardwareAcceleration
  }
}

function firstNonEmptyValue(params: URLSearchParams, keys: string[]): string | null {
  for (const key of keys) {
    const value = params.get(key)?.trim()
    if (value) {
      return value
    }
  }
  return null
}

function parseMediaVideoHardwareAcceleration(value: string | null): MediaVideoHardwareAcceleration | undefined {
  if (value === 'prefer-hardware' || value === 'prefer-software' || value === 'no-preference') {
    return value
  }
  return undefined
}
