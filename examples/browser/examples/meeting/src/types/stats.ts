export interface SidebarStatsSample {
  timestamp: number
  videoBitrateKbps?: number | null
  screenShareBitrateKbps?: number | null
  audioBitrateKbps?: number | null
  localCameraCaptureToEncodeDoneMs?: number | null
  localCameraEncodeQueueSize?: number | null
  localCameraSendQueueWaitMs?: number | null
  localCameraSendActiveMs?: number | null
  localCameraSendObjectMs?: number | null
  localCameraSendSerializeMs?: number | null
  localCameraSendEndOfGroupMs?: number | null
  localCameraSendQueueDepth?: number | null
  localCameraSendObjectBytes?: number | null
  localCameraSendObjectCount?: number | null
  localCameraSendAliasCount?: number | null
  localCameraSendKeyframe?: number | null
  localScreenShareCaptureToEncodeDoneMs?: number | null
  localScreenShareEncodeQueueSize?: number | null
  localScreenShareSendQueueWaitMs?: number | null
  localScreenShareSendActiveMs?: number | null
  localScreenShareSendObjectMs?: number | null
  localScreenShareSendSerializeMs?: number | null
  localScreenShareSendEndOfGroupMs?: number | null
  localScreenShareSendQueueDepth?: number | null
  localScreenShareSendObjectBytes?: number | null
  localScreenShareSendObjectCount?: number | null
  localScreenShareSendAliasCount?: number | null
  localScreenShareSendKeyframe?: number | null
  videoKeyframeIntervalFrames?: number | null
  screenShareKeyframeIntervalFrames?: number | null
  videoReceiveLatencyMs?: number | null
  screenShareReceiveLatencyMs?: number | null
  audioReceiveLatencyMs?: number | null
  videoRenderLatencyMs?: number | null
  screenShareRenderLatencyMs?: number | null
  audioRenderLatencyMs?: number | null
  videoReceiveToDecodeMs?: number | null
  videoReceiveToRenderMs?: number | null
  screenShareReceiveToDecodeMs?: number | null
  screenShareReceiveToRenderMs?: number | null
  videoPacingEffectiveIntervalMs?: number | null
  videoPacingBufferedFrames?: number | null
  videoDecodeQueueSize?: number | null
  videoPacingTargetFrames?: number | null
  screenSharePacingEffectiveIntervalMs?: number | null
  screenSharePacingBufferedFrames?: number | null
  screenShareDecodeQueueSize?: number | null
  screenSharePacingTargetFrames?: number | null
  audioPlaybackQueueMs?: number | null
  videoRenderingRateFps?: number | null
  screenShareRenderingRateFps?: number | null
  audioRenderingRateFps?: number | null
}

export interface SidebarMemberStats {
  memberId: string
  memberName: string
  samples: SidebarStatsSample[]
}
