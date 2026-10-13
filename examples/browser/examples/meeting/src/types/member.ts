export interface SubscriptionState {
  isSubscribing: boolean
  isSubscribed: boolean
  subscribeId?: bigint
}

// Local member (self)
export interface LocalMember {
  id: string
  name: string
}

// Remote member
export interface RemoteMember {
  id: string
  name: string
  trackNamespace: string[]
  subscribedTracks: {
    chat: SubscriptionState
    video: SubscriptionState
    screenshare: SubscriptionState
    audio: SubscriptionState
  }
}
