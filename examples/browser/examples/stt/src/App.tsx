import { useCallback, useMemo, useRef, useState } from 'react'
import { PipelineGraph } from './PipelineGraph'
import { TurnTable } from './TurnTable'
import { VoicePipelineClient } from './client'
import { ReplyPlayer } from './replyPlayer'
import type { PipelineObject, Topology, Turn, TurnEvent } from './types'

const DEFAULT_URL = 'https://127.0.0.1:4433'
const DEFAULT_NAMESPACE = 'stt/demo'

function applyTurnEvent(turn: Turn, event: TurnEvent): Turn {
  if (event.state === 'start') {
    return { ...turn, running: event.stage }
  }
  if (event.state === 'failed') {
    return { ...turn, running: undefined, failed: event.stage }
  }
  const stages = { ...turn.stages, [event.stage]: event.elapsed_ms }
  return {
    ...turn,
    stages,
    running: undefined,
    transcript: event.stage === 'stt' ? event.text : turn.transcript,
    reply: event.stage === 'llm' ? event.text : turn.reply,
    utteranceSec: event.detail?.utterance_sec ?? turn.utteranceSec
  }
}

export function App() {
  const [url, setUrl] = useState(new URLSearchParams(location.search).get('url') ?? DEFAULT_URL)
  const [namespace, setNamespace] = useState(DEFAULT_NAMESPACE)
  const [status, setStatus] = useState('idle')
  const [topology, setTopology] = useState<Topology | null>(null)
  const [turns, setTurns] = useState<Turn[]>([])
  const [audioObjects, setAudioObjects] = useState(0)
  const [running, setRunning] = useState(false)
  const clientRef = useRef<VoicePipelineClient | null>(null)
  const playerRef = useRef<ReplyPlayer | null>(null)

  const updateTurn = useCallback((id: number, update: (turn: Turn) => Turn) => {
    setTurns((previous) => {
      const index = previous.findIndex((turn) => turn.turn === id)
      const current = index >= 0 ? previous[index] : { turn: id, stages: {} }
      const next = update(current)
      if (index >= 0) {
        return previous.map((turn, at) => (at === index ? next : turn))
      }
      return [...previous, next]
    })
  }, [])

  const onPipelineObject = useCallback(
    (object: PipelineObject) => {
      if (object.type === 'topology') {
        setTopology(object)
        return
      }
      if (object.type === 'reply_audio') {
        updateTurn(object.turn, (turn) => ({ ...turn, replyPackets: object.packets }))
        playerRef.current?.expect(object.turn, object.packets)
        return
      }
      updateTurn(object.turn, (turn) => {
        const updated = applyTurnEvent(turn, object)
        const audio = object.detail?.audio
        if (object.stage !== 'vad' || !audio) {
          return updated
        }
        const sentAt = clientRef.current?.sentAtOf(audio.group_id, audio.object_id)
        const utteranceMs = (object.detail?.utterance_sec ?? 0) * 1000
        return {
          ...updated,
          speechStartedAt: sentAt === undefined ? undefined : sentAt - utteranceMs
        }
      })
    },
    [updateTurn]
  )

  const start = useCallback(async () => {
    setRunning(true)
    setTurns([])
    const player = new ReplyPlayer((turn, at) =>
      updateTurn(turn, (existing) => ({
        ...existing,
        playbackMs: existing.firstPacketAt === undefined ? undefined : at - existing.firstPacketAt,
        totalMs: existing.speechStartedAt === undefined ? undefined : at - existing.speechStartedAt
      }))
    )
    const client = new VoicePipelineClient({
      onPipelineObject,
      onReplyPacket: (turn, packet) => {
        updateTurn(turn, (existing) => ({
          ...existing,
          firstPacketAt: existing.firstPacketAt ?? performance.now()
        }))
        player.feed(turn, packet)
      },
      onAudioObjectsSent: setAudioObjects,
      onStatus: setStatus
    })
    playerRef.current = player
    clientRef.current = client
    try {
      await client.start(url, namespace)
    } catch (error) {
      setStatus(`failed: ${error instanceof Error ? error.message : String(error)}`)
      await stop()
    }
  }, [namespace, onPipelineObject, updateTurn, url])

  const stop = useCallback(async () => {
    await clientRef.current?.stop()
    await playerRef.current?.close()
    clientRef.current = null
    playerRef.current = null
    setRunning(false)
    setAudioObjects(0)
  }, [])

  const currentTurn = useMemo(() => turns[turns.length - 1], [turns])

  return (
    <div
      style={{
        padding: 20,
        fontFamily: 'system-ui, sans-serif',
        color: '#e2e8f0',
        background: '#020617',
        minHeight: '100vh'
      }}
    >
      <h1 style={{ fontSize: 20 }}>MoQT Voice Pipeline</h1>
      <p className="hint" style={{ color: '#94a3b8', fontSize: 13 }}>
        The microphone is published as an Opus MoQT track. The server sends its pipeline structure and every stage
        transition back on the <code>pipeline</code> track, and the spoken reply on the <code>reply</code> track. One
        conversation turn is one MoQT group id.
      </p>

      <div style={{ display: 'flex', gap: 8, alignItems: 'center', margin: '12px 0' }}>
        <input
          value={url}
          onChange={(event) => setUrl(event.target.value)}
          disabled={running}
          style={{ width: 260 }}
          data-testid="voice-url-input"
        />
        <input
          value={namespace}
          onChange={(event) => setNamespace(event.target.value)}
          disabled={running}
          style={{ width: 160 }}
          data-testid="voice-namespace-input"
        />
        <button onClick={running ? stop : start} data-testid="voice-toggle-button">
          {running ? 'Stop' : 'Connect & start microphone'}
        </button>
        <span data-testid="voice-status" style={{ fontSize: 13, color: '#94a3b8' }}>
          {status}
        </span>
      </div>

      <PipelineGraph topology={topology} turn={currentTurn} audioObjects={audioObjects} />

      <h2 style={{ fontSize: 16, marginTop: 20 }}>Turns</h2>
      <div data-testid="voice-turns">
        <TurnTable turns={turns} />
      </div>
    </div>
  )
}
