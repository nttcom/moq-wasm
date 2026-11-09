/// The encoder names each rendition's codec with the level of its picture
/// size, so renditions that differ only in level are one codec to a decoder. An H.264 encoder also sets the constraint flags
/// by the level (`avc1.42D00B` and `avc1.42C00A`), so only its profile counts.
export function sameCodecIgnoringLevel(a: string | undefined, b: string | undefined): boolean {
  return a === b || (a !== undefined && b !== undefined && codecWithoutLevel(a) === codecWithoutLevel(b))
}

function codecWithoutLevel(codec: string): string {
  const [family, ...fields] = codec.split('.')
  switch (family) {
    case 'avc1':
    case 'avc3':
      return `${family}.${fields[0]?.slice(0, 2)}`
    case 'vp09':
      return [family, fields[0], ...fields.slice(2)].join('.')
    case 'av01':
      return [family, fields[0], fields[1]?.slice(-1), ...fields.slice(2)].join('.')
    default:
      return codec
  }
}
