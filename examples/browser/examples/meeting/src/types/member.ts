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

/// A remote member; its media plays on `RemotePlayers`, its chat on the session.
export interface RemoteMember {
  id: string
  name: string
  trackNamespace: string[]
  chat: SubscriptionState
}
