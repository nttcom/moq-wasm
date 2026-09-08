export type StageId = 'vad' | 'stt' | 'llm' | 'tts' | 'turn'
export type StageState = 'start' | 'done' | 'failed'

export type TopologyNode = {
  id: string
  kind: 'client' | 'track' | 'stage'
  label: string
  impl?: string
}

export type Topology = {
  type: 'topology'
  nodes: TopologyNode[]
  edges: [string, string][]
}

export type TurnEvent = {
  type: 'turn_event'
  track: string
  turn: number
  stage: StageId
  state: StageState
  elapsed_ms?: number
  text?: string
  detail?: {
    utterance_sec?: number
    audio?: { group_id: number; object_id: number }
    speech_sec?: number
  }
  at: number
}

export type ReplyAudioAnnouncement = {
  type: 'reply_audio'
  turn: number
  packets: number
  sec: number
}

export type PipelineObject = Topology | TurnEvent | ReplyAudioAnnouncement

/** What the UI shows for one conversation turn. The stage durations are
 * measured on the server; `playbackMs` and `totalMs` are measured here. */
export type Turn = {
  turn: number
  transcript?: string
  reply?: string
  stages: Partial<Record<StageId, number>>
  running?: StageId
  failed?: StageId
  utteranceSec?: number
  /** Browser clock (`performance.now()`) when this turn's speech began. */
  speechStartedAt?: number
  replyPackets?: number
  firstPacketAt?: number
  playbackMs?: number
  totalMs?: number
}
