export const OBJECT_STATUS_END_OF_GROUP = 3
const OBJECT_STATUS_END_OF_TRACK = 4

export function isTerminalStatus(status: number | undefined): boolean {
  return status === OBJECT_STATUS_END_OF_GROUP || status === OBJECT_STATUS_END_OF_TRACK
}
