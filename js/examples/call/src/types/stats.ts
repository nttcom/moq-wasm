export interface SidebarStatsSample {
  timestamp: number
  videoBitrateKbps?: number | null
  screenShareBitrateKbps?: number | null
  audioBitrateKbps?: number | null
  videoKeyframeIntervalFrames?: number | null
  screenShareKeyframeIntervalFrames?: number | null
  videoReceiveLatencyMs?: number | null
  screenShareReceiveLatencyMs?: number | null
  audioReceiveLatencyMs?: number | null
  videoRenderLatencyMs?: number | null
  screenShareRenderLatencyMs?: number | null
  audioRenderLatencyMs?: number | null
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
