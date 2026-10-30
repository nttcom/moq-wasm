export const RequestErrorCode = {
  InternalError: 0x0n,
  Timeout: 0x2n,
  NotSupported: 0x3n,
  TrackDoesNotExist: 0x4n,
  InvalidRange: 0x5n,
  NoObjects: 0x6n,
  UnknownStatusInRange: 0x8n
} as const

/// A request the relay rejected, with the draft-14 error code.
export class RequestError extends Error {
  readonly errorCode: bigint

  constructor(messageType: string, response: { errorCode: bigint; reasonPhrase: string }) {
    super(`${messageType} ${response.errorCode}: ${response.reasonPhrase}`)
    this.errorCode = response.errorCode
  }
}
