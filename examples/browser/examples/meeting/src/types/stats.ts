import type { RemotePlaybackStats } from '../media/remotePlayers'

export type RemoteStatsSample = RemotePlaybackStats & { timestamp: number }
