export type LocHeader = {
  extensions: LocHeaderExtension[]
}

export type LocHeaderExtension =
  | { type: 'captureTimestamp'; value: { microsSinceUnixEpoch: number } }
  | { type: 'videoConfig'; value: { data: Uint8Array } }
  | { type: 'videoFrameMarking'; value: { data: Uint8Array } }
  | { type: 'audioLevel'; value: { level: number } }
  | { type: 'unknown'; value: { id: number; value: LocHeaderValue } }

export type LocHeaderValue = { evenBytes: Uint8Array } | { oddVarint: number }

export type LocMetadata = {
  captureTimestampMicros?: number
  videoConfig?: Uint8Array
  videoFrameMarking?: Uint8Array
  audioLevel?: number
}

export function buildLocHeader(meta: LocMetadata): LocHeader {
  const extensions: LocHeaderExtension[] = []
  if (typeof meta.captureTimestampMicros === 'number') {
    extensions.push({
      type: 'captureTimestamp',
      value: { microsSinceUnixEpoch: meta.captureTimestampMicros }
    })
  }
  if (meta.videoConfig) {
    extensions.push({
      type: 'videoConfig',
      value: { data: meta.videoConfig }
    })
  }
  if (meta.videoFrameMarking) {
    extensions.push({
      type: 'videoFrameMarking',
      value: { data: meta.videoFrameMarking }
    })
  }
  if (typeof meta.audioLevel === 'number') {
    extensions.push({
      type: 'audioLevel',
      value: { level: meta.audioLevel }
    })
  }
  return { extensions }
}

export function readLocHeader(header?: LocHeader): LocMetadata {
  if (!header) {
    return {}
  }
  const meta: LocMetadata = {}
  for (const ext of header.extensions) {
    switch (ext.type) {
      case 'captureTimestamp':
        meta.captureTimestampMicros = ext.value.microsSinceUnixEpoch
        break
      case 'videoConfig':
        meta.videoConfig = ext.value.data
        break
      case 'videoFrameMarking':
        meta.videoFrameMarking = ext.value.data
        break
      case 'audioLevel':
        meta.audioLevel = ext.value.level
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
