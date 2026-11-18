export type TranscriptTurn = { role: 'user' | 'assistant'; text: string }

/// The turns of one object of the bot's transcript: draft-ietf-moq-msf-02 §8.2 makes each object a
/// JSON array of records, each `{"T": milliseconds, "data": {"role", "text"}}`. Anything else is
/// skipped.
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
    const data = (record as { data?: { role?: unknown; text?: unknown } } | null)?.data
    const role = data?.role
    const text = data?.text
    return (role === 'user' || role === 'assistant') && typeof text === 'string' && text.length > 0
      ? [{ role, text }]
      : []
  })
}
