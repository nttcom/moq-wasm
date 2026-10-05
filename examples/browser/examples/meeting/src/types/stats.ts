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
  bufferMs?: number | null
  targetBufferMs?: number | null
  outputLatencyMs?: number | null
  arrivalSpreadMs?: number | null
  viewerDelayMs?: number | null
  syncOffsetMs?: number | null
}

export interface SidebarMemberStats {
  memberId: string
  memberName: string
  samples: SidebarStatsSample[]
}
