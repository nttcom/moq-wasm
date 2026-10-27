import type { RemotePlaybackStats } from '../media/mediaSubscriber'

export interface VideoSendTiming {
  captureToEncodeDoneMs: number | null
  encodeQueueSize: number
  queueWaitMs: number
  sendActiveMs: number
  objectSendMs: number
  queueDepth: number
  objectBytes: number
}

export interface LocalSendStats {
  cameraBitrateKbps: number | null
  screenShareBitrateKbps: number | null
  audioBitrateKbps: number | null
  cameraSend: VideoSendTiming | null
  screenShareSend: VideoSendTiming | null
}

export const NO_LOCAL_SEND_STATS: LocalSendStats = {
  cameraBitrateKbps: null,
  screenShareBitrateKbps: null,
  audioBitrateKbps: null,
  cameraSend: null,
  screenShareSend: null
}

export type LocalStatsSample = LocalSendStats & { timestamp: number }

export type RemoteStatsSample = RemotePlaybackStats & { timestamp: number }
