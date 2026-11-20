import type { MoqSession } from '../core/session'

/// A namespace stays announced until its session closes, so it is never removed.
const announced = new WeakMap<MoqSession, Set<string>>()

export function noteOwnBroadcast(session: MoqSession, namespace: string[]): void {
  const namespaces = announced.get(session) ?? new Set<string>()
  namespaces.add(key(namespace))
  announced.set(session, namespaces)
}

export function isOwnBroadcast(session: MoqSession, namespace: string[]): boolean {
  return announced.get(session)?.has(key(namespace)) ?? false
}

function key(namespace: string[]): string {
  return JSON.stringify(namespace)
}
