export interface RemoteMediaStreams {
  videoStream?: MediaStream | null
  audioStream?: MediaStream | null
  videoBitrateKbps?: number
  audioBitrateKbps?: number
  videoLatencyRenderMs?: number
  videoLatencyReceiveMs?: number
  audioLatencyRenderMs?: number
  audioLatencyReceiveMs?: number
}
