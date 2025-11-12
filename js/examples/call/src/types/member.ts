// Track information
export interface TrackState {
  isAnnounced: boolean
  trackNamespace?: string[]
}

export interface SubscriptionState {
  isSubscribing: boolean
  isSubscribed: boolean
  subscribeId?: bigint
}

// Local member (self)
export interface LocalMember {
  id: string
  name: string
  publishedTracks: {
    chat: boolean
    video: boolean
    audio: boolean
  }
}

// Remote member
export interface RemoteMember {
  id: string
  name: string
  announcedTracks: {
    chat: TrackState
    video: TrackState
    audio: TrackState
  }
  subscribedTracks: {
    chat: SubscriptionState
    video: SubscriptionState
    audio: SubscriptionState
  }
}
