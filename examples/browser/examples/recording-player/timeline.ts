export type TimelineEvent = { at: number; source: string; kind: string; speaker: string; text: string }

export type IndexedChunk = {
  first: { group: string; object: string } | null
  last: { group: string; object: string } | null
  capture_start_us: string | null
  capture_end_us: string | null
  data_size: string
}

export function chunkIndexAt(chunks: IndexedChunk[], at: number): number {
  const index = chunks.findLastIndex((chunk) => Number(chunk.capture_start_us) <= at)
  return Math.max(0, index)
}

export function eventRecords(payload: Uint8Array, source: string, kind: string): TimelineEvent[] {
  const records: unknown = JSON.parse(new TextDecoder().decode(payload))
  if (!Array.isArray(records)) throw new Error('Invalid event timeline')
  return records.map((record) => {
    if (
      !Number.isSafeInteger(record?.t) ||
      !Number.isSafeInteger(record.t * 1000) ||
      typeof record?.data?.text !== 'string'
    ) {
      throw new Error('Event has no source time or text')
    }
    if (kind === 'transcript' && typeof record.data.speaker !== 'string') throw new Error('Transcript has no speaker')
    return {
      at: record.t * 1000,
      source,
      kind,
      speaker: typeof record.data.speaker === 'string' ? record.data.speaker : source.split('/').at(-1)!,
      text: record.data.text
    }
  })
}

export function mergeEvents(events: TimelineEvent[]): TimelineEvent[] {
  const merged = new Map<string, TimelineEvent>()
  for (const event of events) {
    const key = JSON.stringify([
      event.source,
      event.kind,
      event.at,
      event.speaker,
      event.kind === 'chat' ? event.text : ''
    ])
    merged.set(key, event)
  }
  return [...merged.values()].sort((a, b) => a.at - b.at)
}

export function isKeyframe(codec: string, data: Uint8Array): boolean {
  if (codec.startsWith('avc1') || codec.startsWith('avc3')) {
    for (let i = 0; i + 3 < data.length; i++) {
      if (data[i] === 0 && data[i + 1] === 0) {
        const start = data[i + 2] === 1 ? i + 3 : data[i + 2] === 0 && data[i + 3] === 1 ? i + 4 : -1
        if (start >= 0 && start < data.length && (data[start] & 31) === 5) return true
      }
    }
    return false
  }
  if (codec === 'vp8')
    return data.length >= 6 && (data[0] & 1) === 0 && data[3] === 0x9d && data[4] === 1 && data[5] === 0x2a
  if (codec.startsWith('vp09')) {
    // VP9 uncompressed header: frame_marker (2 bits), profile (2 bits, then a reserved bit in profile 3), show_existing_frame, frame_type (0 = key frame).
    const profile = ((data[0] >> 5) & 1) | (((data[0] >> 4) & 1) << 1)
    const shift = profile === 3 ? 1 : 0
    return (
      data.length > 0 &&
      data[0] >> 6 === 2 &&
      ((data[0] >> (3 - shift)) & 1) === 0 &&
      ((data[0] >> (2 - shift)) & 1) === 0
    )
  }
  throw new Error(`Access-point detection is unavailable for ${codec}`)
}

export function keyframeDetector(codec: string): (data: Uint8Array) => boolean {
  if (!codec.startsWith('av01')) return (data) => isKeyframe(codec, data)
  let reducedStillPicture: boolean | undefined
  return (data) => {
    let position = 0
    while (position < data.length) {
      const header = data[position++]
      if (header & 0x81) throw new Error('Invalid AV1 OBU header')
      const type = (header >> 3) & 15
      if (header & 4) position++
      let size = data.length - position
      if (header & 2) {
        size = 0
        let complete = false
        for (let byte = 0; byte < 8 && position < data.length; byte++) {
          const value = data[position++]
          size += (value & 127) * 2 ** (byte * 7)
          if (!(value & 128)) {
            complete = true
            break
          }
        }
        if (!complete || !Number.isSafeInteger(size)) throw new Error('Invalid AV1 OBU size')
      }
      const end = position + size
      if (end > data.length || position > data.length) throw new Error('Truncated AV1 OBU')
      if (type === 1 || type === 3 || type === 6) {
        if (!size) throw new Error('Empty AV1 frame or sequence header')
        if (type === 1) reducedStillPicture = (data[position] & 8) !== 0
        else return reducedStillPicture !== undefined && (reducedStillPicture || data[position] >> 5 === 0)
      }
      position = end
    }
    return false
  }
}
