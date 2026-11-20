export function isHevcCodec(codec: string): boolean {
  return codec.startsWith('hvc1') || codec.startsWith('hev1')
}
