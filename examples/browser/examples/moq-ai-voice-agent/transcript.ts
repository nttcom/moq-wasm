/// `turn` names an answer of the bot: its records carry the answer so far, each replacing the last.
export type TranscriptTurn = { role: 'user' | 'assistant'; text: string; turn?: number }

/// The turns of one object of the bot's transcript: draft-ietf-moq-msf-02 §8.2 makes each object a
/// JSON array of records, each `{"T": milliseconds, "data": {"role", "text", "turn"?}}`.
export function transcriptTurns(payload: Uint8Array): TranscriptTurn[] {
  let records: unknown
  try {
    records = JSON.parse(new TextDecoder().decode(payload))
  } catch {
    return []
  }
  if (!Array.isArray(records)) {
    return []
  }
  return records.flatMap((record) => {
    const data = (record as { data?: { role?: unknown; text?: unknown; turn?: unknown } } | null)?.data
    const role = data?.role
    const text = data?.text
    if ((role !== 'user' && role !== 'assistant') || typeof text !== 'string' || text.length === 0) {
      return []
    }
    return [{ role, text, ...(Number.isInteger(data?.turn) && { turn: data?.turn as number }) }]
  })
}
