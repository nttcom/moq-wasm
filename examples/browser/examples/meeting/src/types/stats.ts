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

export interface LocalStatsSample {
  timestamp: number
  cameraBitrateKbps: number | null
  screenShareBitrateKbps: number | null
  audioBitrateKbps: number | null
  cameraSend: VideoSendTiming | null
  screenShareSend: VideoSendTiming | null
}

export type RemoteStatsSample = RemotePlaybackStats & { timestamp: number }
