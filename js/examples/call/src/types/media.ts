export type JitterBufferEvent = 'push' | 'pop'

export interface JitterBufferSnapshot {
  bufferedFrames: number
  capacityFrames: number
  lastEvent: JitterBufferEvent
  sequence: number
  updatedAtMs: number
}

export interface RemoteMediaStreams {
  videoStream?: MediaStream | null
  screenShareStream?: MediaStream | null
  audioStream?: MediaStream | null
  videoBitrateKbps?: number
  screenShareBitrateKbps?: number
  audioBitrateKbps?: number
  videoKeyframeIntervalFrames?: number
  screenShareKeyframeIntervalFrames?: number
  videoRenderingRateFps?: number
  screenShareRenderingRateFps?: number
  audioRenderingRateFps?: number
  videoLatencyRenderMs?: number
  videoLatencyReceiveMs?: number
  screenShareLatencyRenderMs?: number
  screenShareLatencyReceiveMs?: number
  audioLatencyRenderMs?: number
  audioLatencyReceiveMs?: number
  audioPlaybackQueueMs?: number
  videoCodec?: string
  videoWidth?: number
  videoHeight?: number
  screenShareCodec?: string
  screenShareWidth?: number
  screenShareHeight?: number
  videoJitterBuffer?: JitterBufferSnapshot
  screenShareJitterBuffer?: JitterBufferSnapshot
  audioJitterBuffer?: JitterBufferSnapshot
}
