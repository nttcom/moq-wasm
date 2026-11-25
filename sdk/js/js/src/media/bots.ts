const BOTS = 'bots'

/// The namespace a bot of `botType` publishes at in `room`: `[...room, 'bots', botType]`.
export function botNamespace(room: string[], botType: string): string[] {
  return [...room, BOTS, botType]
}

/// The bot type when `namespace` is exactly `[...room, 'bots', <type>]`,
/// otherwise `undefined`: a member's namespace, one deeper under `bots` and one
/// of another room are not bots of `room`.
export function botTypeOf(room: string[], namespace: string[]): string | undefined {
  if (namespace.length !== room.length + 2 || namespace[room.length] !== BOTS) {
    return undefined
  }
  return room.every((element, index) => namespace[index] === element) ? namespace[room.length + 1] : undefined
}
