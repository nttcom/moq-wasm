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
  videoReceiveToDecodeMs?: number | null
  videoReceiveToRenderMs?: number | null
  videoPacingIntervalMs?: number
  videoPacingEffectiveIntervalMs?: number
  videoPacingBufferedFrames?: number
  videoDecodeQueueSize?: number
  videoPacingTargetFrames?: number
  videoDecodingGroupId?: string
  videoDecodingObjectId?: string
  videoDecodingChunkType?: string
  videoDecodingPhase?: 'submit' | 'output' | 'error'
  screenShareLatencyRenderMs?: number
  screenShareLatencyReceiveMs?: number
  screenShareReceiveToDecodeMs?: number | null
  screenShareReceiveToRenderMs?: number | null
  screenSharePacingIntervalMs?: number
  screenSharePacingEffectiveIntervalMs?: number
  screenSharePacingBufferedFrames?: number
  screenShareDecodeQueueSize?: number
  screenSharePacingTargetFrames?: number
  screenShareDecodingGroupId?: string
  screenShareDecodingObjectId?: string
  screenShareDecodingChunkType?: string
  screenShareDecodingPhase?: 'submit' | 'output' | 'error'
  audioLatencyRenderMs?: number
  audioLatencyReceiveMs?: number
  audioPlaybackQueueMs?: number
  videoCodec?: string
  videoWidth?: number
  videoHeight?: number
  videoDecoderDescriptionLength?: number
  videoDecoderAvcFormat?: 'annexb' | 'avc'
  videoDecoderHardwareAcceleration?: HardwareAcceleration
  videoDecoderOptimizeForLatency?: boolean
  screenShareCodec?: string
  screenShareWidth?: number
  screenShareHeight?: number
  screenShareDecoderDescriptionLength?: number
  screenShareDecoderAvcFormat?: 'annexb' | 'avc'
  screenShareDecoderHardwareAcceleration?: HardwareAcceleration
  screenShareDecoderOptimizeForLatency?: boolean
  videoJitterBuffer?: JitterBufferSnapshot
  screenShareJitterBuffer?: JitterBufferSnapshot
  audioJitterBuffer?: JitterBufferSnapshot
}
