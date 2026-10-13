// Uint8Array base64 methods (ES2026) are not yet in the bundled TS lib definitions.
interface Uint8Array<TArrayBuffer extends ArrayBufferLike = ArrayBufferLike> {
  toBase64(): string
}

interface Uint8ArrayConstructor {
  fromBase64(base64: string): Uint8Array<ArrayBuffer>
}
