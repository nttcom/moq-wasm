export type LocValue = { varint: number } | { bytes: Uint8Array }

export type LocExtension = { id: number; value: LocValue }

export type LocHeader = LocExtension[]

export const CAPTURE_TIMESTAMP_ID = 2
export const VIDEO_FRAME_MARKING_ID = 4
export const AUDIO_LEVEL_ID = 6
export const VIDEO_CONFIG_ID = 13

export type LocMetadata = {
  captureTimestampMicros?: number
  videoConfig?: Uint8Array
  videoFrameMarking?: number
  audioLevel?: number
}

export function buildLocHeader(meta: LocMetadata): LocHeader {
  const extensions: LocHeader = []
  if (typeof meta.captureTimestampMicros === 'number') {
    extensions.push({ id: CAPTURE_TIMESTAMP_ID, value: { varint: meta.captureTimestampMicros } })
  }
  if (meta.videoConfig) {
    extensions.push({ id: VIDEO_CONFIG_ID, value: { bytes: meta.videoConfig } })
  }
  if (typeof meta.videoFrameMarking === 'number') {
    extensions.push({ id: VIDEO_FRAME_MARKING_ID, value: { varint: meta.videoFrameMarking } })
  }
  if (typeof meta.audioLevel === 'number') {
    extensions.push({ id: AUDIO_LEVEL_ID, value: { varint: meta.audioLevel } })
  }
  return extensions
}

export function readLocHeader(header?: LocHeader): LocMetadata {
  const meta: LocMetadata = {}
  for (const ext of header ?? []) {
    if ('bytes' in ext.value) {
      if (ext.id === VIDEO_CONFIG_ID) {
        meta.videoConfig = ext.value.bytes
      }
      continue
    }
    switch (ext.id) {
      case CAPTURE_TIMESTAMP_ID:
        meta.captureTimestampMicros = ext.value.varint
        break
      case VIDEO_FRAME_MARKING_ID:
        meta.videoFrameMarking = ext.value.varint
        break
      case AUDIO_LEVEL_ID:
        meta.audioLevel = ext.value.varint
        break
      default:
        break
    }
  }
  return meta
}

export function arrayBufferToUint8Array(buffer?: ArrayBuffer): Uint8Array | undefined {
  if (!buffer) {
    return undefined
  }
  return new Uint8Array(buffer)
}

export function bytesToBase64(bytes: Uint8Array): string {
  let binary = ''
  for (let i = 0; i < bytes.length; i++) {
    binary += String.fromCharCode(bytes[i])
  }
  return btoa(binary)
}
