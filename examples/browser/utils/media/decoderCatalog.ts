export type AudioCatalogTrackLike = {
  codec?: string
  samplerate?: number
  channelConfig?: string
  initData?: string
}

export type VideoCatalogTrackLike = {
  codec?: string
  framerate?: number
  initData?: string
  avcFormat?: 'annexb' | 'avc'
}

export type AudioDecoderCatalogMessage = {
  type: 'catalog'
  codec?: string
  sampleRate?: number
  channels?: number
  descriptionBase64?: string
}

export type VideoDecoderCatalogMessage = {
  type: 'catalog'
  codec?: string
  framerate?: number
  descriptionBase64?: string
  avcFormat?: 'annexb' | 'avc'
}

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

export function buildAudioDecoderCatalogMessage(track: AudioCatalogTrackLike): AudioDecoderCatalogMessage | null {
  const channels = parseAudioChannelCount(track.channelConfig)
  if (!track.codec && typeof track.samplerate !== 'number' && channels === undefined && !track.initData) {
    return null
  }
  return {
    type: 'catalog',
    codec: track.codec,
    sampleRate: track.samplerate,
    channels,
    descriptionBase64: track.initData
  }
}

export function buildVideoDecoderCatalogMessage(track: VideoCatalogTrackLike): VideoDecoderCatalogMessage | null {
  if (!track.codec && typeof track.framerate !== 'number' && !track.initData && !track.avcFormat) {
    return null
  }
  return {
    type: 'catalog',
    codec: track.codec,
    framerate: track.framerate,
    descriptionBase64: track.initData,
    avcFormat: track.avcFormat
  }
}

export function postAudioCatalogToWorker(worker: Worker, track: AudioCatalogTrackLike): boolean {
  const message = buildAudioDecoderCatalogMessage(track)
  if (!message) {
    return false
  }
  worker.postMessage(message)
  return true
}

export function postVideoCatalogToWorker(worker: Worker, track: VideoCatalogTrackLike): boolean {
  const message = buildVideoDecoderCatalogMessage(track)
  if (!message) {
    return false
  }
  worker.postMessage(message)
  return true
}
