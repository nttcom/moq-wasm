/// draft-ietf-moq-loc-04 §2.1.3: a length of 1 reads as a 4-byte start code.
/// A decoder configured with an avcC description reads length prefixes, so an
/// Annex-B payload is rewritten with the description's NAL length size.
export function withLengthPrefixes(payload: Uint8Array, avcc: Uint8Array): Uint8Array {
  if (!(payload[0] === 0 && payload[1] === 0 && payload[2] === 0 && payload[3] === 1)) {
    return payload
  }
  const lengthSize = (avcc[4] & 0x03) + 1
  const nals = nalUnits(payload)
  const out = new Uint8Array(nals.reduce((total, nal) => total + lengthSize + nal.length, 0))
  let offset = 0
  for (const nal of nals) {
    for (let index = 0; index < lengthSize; index++) {
      out[offset + index] = (nal.length >>> (8 * (lengthSize - 1 - index))) & 0xff
    }
    out.set(nal, offset + lengthSize)
    offset += lengthSize + nal.length
  }
  return out
}

function nalUnits(annexb: Uint8Array): Uint8Array[] {
  const starts: number[] = []
  for (let index = 0; index + 2 < annexb.length; index++) {
    if (annexb[index] === 0 && annexb[index + 1] === 0 && annexb[index + 2] === 1) {
      starts.push(index + 3)
      index += 2
    }
  }
  return starts
    .map((start, index) => {
      let end = index + 1 < starts.length ? starts[index + 1] - 3 : annexb.length
      while (end > start && annexb[end - 1] === 0) {
        end--
      }
      return annexb.subarray(start, end)
    })
    .filter((nal) => nal.length > 0)
}
