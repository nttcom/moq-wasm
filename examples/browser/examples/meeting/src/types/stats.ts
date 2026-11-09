import type { RemotePlaybackStats } from '../media/mediaSubscriber'

export type RemoteStatsSample = RemotePlaybackStats & { timestamp: number }
