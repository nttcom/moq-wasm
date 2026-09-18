type AudioCatalogTrackLike = {
  codec?: string
  samplerate?: number
  channelConfig?: string
  initData?: string
}

type VideoCatalogTrackLike = {
  codec?: string
  framerate?: number
  initData?: string
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

export function postAudioCatalogToWorker(worker: Worker, track: AudioCatalogTrackLike): void {
  worker.postMessage({
    type: 'catalog',
    codec: track.codec,
    sampleRate: track.samplerate,
    channels: parseAudioChannelCount(track.channelConfig),
    descriptionBase64: track.initData
  })
}

export function postVideoCatalogToWorker(worker: Worker, track: VideoCatalogTrackLike): void {
  worker.postMessage({
    type: 'catalog',
    codec: track.codec,
    framerate: track.framerate,
    descriptionBase64: track.initData,
    avcFormat: track.avcFormat
  })
}
