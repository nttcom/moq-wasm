export interface ChatMessage {
  sender: string
  trackNamespace: string[]
  groupId: bigint
  text: string
  timestamp: number
  isLocal: boolean
}
