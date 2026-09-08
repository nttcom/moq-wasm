import type { Turn } from './types'

/** The VAD's duration is the utterance itself, shown once as `speech`. */
const SERVER_STAGES = ['stt', 'llm', 'tts'] as const

function ms(value: number | undefined): string {
  return value === undefined ? '—' : `${Math.round(value)}`
}

function responseMs(turn: Turn): number | undefined {
  if (turn.totalMs === undefined || turn.utteranceSec === undefined) {
    return undefined
  }
  return turn.totalMs - turn.utteranceSec * 1000
}

export function TurnTable({ turns }: { turns: Turn[] }) {
  if (!turns.length) {
    return <p style={{ color: '#94a3b8', fontSize: 13 }}>No turn yet. Speak into the microphone.</p>
  }
  return (
    <table style={{ width: '100%', fontSize: 12, borderCollapse: 'collapse' }}>
      <thead>
        <tr style={{ textAlign: 'left', color: '#94a3b8' }}>
          <th>turn / group</th>
          <th>speech</th>
          <th>transcript → reply</th>
          {SERVER_STAGES.map((stage) => (
            <th key={stage}>{stage} ms</th>
          ))}
          <th>playback ms</th>
          <th>after speech ms</th>
          <th>turn ms</th>
        </tr>
      </thead>
      <tbody>
        {turns.map((turn) => (
          <tr key={turn.turn} style={{ borderTop: '1px solid #334155' }}>
            <td>{turn.turn}</td>
            <td>{turn.utteranceSec === undefined ? '—' : `${turn.utteranceSec.toFixed(1)}s`}</td>
            <td style={{ maxWidth: 420 }}>
              <div>{turn.transcript ?? '…'}</div>
              <div style={{ color: '#34d399' }}>{turn.reply ?? ''}</div>
            </td>
            {SERVER_STAGES.map((stage) => (
              <td key={stage}>{ms(turn.stages[stage])}</td>
            ))}
            <td>{ms(turn.playbackMs)}</td>
            <td>{ms(responseMs(turn))}</td>
            <td style={{ fontWeight: 600 }}>{ms(turn.totalMs)}</td>
          </tr>
        ))}
      </tbody>
    </table>
  )
}
