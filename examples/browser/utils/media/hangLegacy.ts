/// hang's legacy container (moq-dev): one QUIC varint holding the presentation
/// timestamp in microseconds, followed by the raw codec bitstream.
export type LegacyFrame = {
  timestampMicros: number
  payload: Uint8Array
}

export function readLegacyFrame(bytes: Uint8Array): LegacyFrame | undefined {
  if (bytes.byteLength === 0) {
    return undefined
  }
  const length = 1 << (bytes[0] >> 6)
  if (bytes.byteLength < length) {
    return undefined
  }
  let value = BigInt(bytes[0] & 0x3f)
  for (let index = 1; index < length; index += 1) {
    value = (value << 8n) | BigInt(bytes[index])
  }
  return { timestampMicros: Number(value), payload: bytes.subarray(length) }
}
