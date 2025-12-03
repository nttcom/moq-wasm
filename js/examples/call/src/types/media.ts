export interface RemoteMediaStreams {
  videoStream?: MediaStream | null
  audioStream?: MediaStream | null
  videoBitrateKbps?: number
  audioBitrateKbps?: number
  videoLatencyMs?: number
  audioLatencyMs?: number
  videoJitterMs?: number
  audioJitterMs?: number
}
