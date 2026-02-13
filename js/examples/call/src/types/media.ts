export interface RemoteMediaStreams {
  videoStream?: MediaStream | null
  screenShareStream?: MediaStream | null
  audioStream?: MediaStream | null
  videoBitrateKbps?: number
  screenShareBitrateKbps?: number
  audioBitrateKbps?: number
  videoLatencyRenderMs?: number
  videoLatencyReceiveMs?: number
  screenShareLatencyRenderMs?: number
  screenShareLatencyReceiveMs?: number
  audioLatencyRenderMs?: number
  audioLatencyReceiveMs?: number
  videoCodec?: string
  videoWidth?: number
  videoHeight?: number
  screenShareCodec?: string
  screenShareWidth?: number
  screenShareHeight?: number
}
