export type LocValue = { varint: number } | { bytes: Uint8Array }

export type LocExtension = { id: number; value: LocValue }

export type LocHeader = LocExtension[]

export const CAPTURE_TIMESTAMP_ID = 2
export const VIDEO_CONFIG_ID = 13

export type LocMetadata = {
  captureTimestampMicros?: number
  videoConfig?: Uint8Array
}

export function buildLocHeader(meta: LocMetadata): LocHeader {
  const extensions: LocHeader = []
  if (typeof meta.captureTimestampMicros === 'number') {
    extensions.push({ id: CAPTURE_TIMESTAMP_ID, value: { varint: meta.captureTimestampMicros } })
  }
  if (meta.videoConfig) {
    extensions.push({ id: VIDEO_CONFIG_ID, value: { bytes: meta.videoConfig } })
  }
  return extensions
}

export function readLocHeader(header?: LocHeader): LocMetadata {
  const meta: LocMetadata = {}
  for (const ext of header ?? []) {
    if (ext.id === VIDEO_CONFIG_ID && 'bytes' in ext.value) {
      meta.videoConfig = ext.value.bytes
    } else if (ext.id === CAPTURE_TIMESTAMP_ID && 'varint' in ext.value) {
      meta.captureTimestampMicros = ext.value.varint
    }
  }
  return meta
}
