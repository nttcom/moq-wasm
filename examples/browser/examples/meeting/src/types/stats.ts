export interface VideoSendTiming {
  captureToEncodeDoneMs: number | null
  encodeQueueSize: number
  queueWaitMs: number
  sendActiveMs: number
  objectSendMs: number
  queueDepth: number
  objectBytes: number
}

export interface SidebarStatsSample {
  timestamp: number
  videoBitrateKbps?: number | null
  screenShareBitrateKbps?: number | null
  audioBitrateKbps?: number | null
  localCameraSend?: VideoSendTiming | null
  localScreenShareSend?: VideoSendTiming | null
  bufferMs?: number | null
  targetBufferMs?: number | null
  outputLatencyMs?: number | null
  arrivalSpreadMs?: number | null
  viewerDelayMs?: number | null
  syncOffsetMs?: number | null
}
