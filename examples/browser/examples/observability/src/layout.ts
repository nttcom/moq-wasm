import type { ClientNode, Topology } from './topology'

export interface Point {
  x: number
  y: number
}

export interface HalfSize {
  w: number
  h: number
}

export const CLIENT_HALF: HalfSize = { w: 50, h: 22 }
export const RELAY_HALF: HalfSize = { w: 62, h: 57 }
export const CENTER: Point = { x: 0, y: 0 }
export const ENVIRONMENT_RADIUS = 230

const FIRST_RING_RADIUS = 345
const RING_SPACING = 70
const CARD_SLOT = 2 * CLIENT_HALF.w + 12
const RELAY_ORBIT = ENVIRONMENT_RADIUS * 0.5
// Two relay cards whose centres are this far apart cannot overlap in any direction.
const RELAY_CLEARANCE = Math.hypot(2 * RELAY_HALF.w, 2 * RELAY_HALF.h)
const START_ANGLE = -Math.PI / 2

export interface Layout {
  relays: Map<string, Point>
  clients: Map<string, Point>
}

function ringCapacity(ring: number): number {
  return Math.floor((2 * Math.PI * (FIRST_RING_RADIUS + ring * RING_SPACING)) / CARD_SLOT)
}

function ringsFor(clientCount: number): number {
  let rings = 1
  while (Array.from({ length: rings }, (_, ring) => ringCapacity(ring)).reduce((a, b) => a + b, 0) < clientCount) {
    rings += 1
  }
  return rings
}

// Pushes neighbouring angles apart until every circular gap is at least `gap`.
function spreadAngles(angles: number[], gap: number): number[] {
  const spread = [...angles]
  for (let pass = 0; pass < 200; pass += 1) {
    let moved = false
    spread.forEach((angle, i) => {
      const j = (i + 1) % spread.length
      const shortfall = gap - (spread[j] + (j === 0 ? 2 * Math.PI : 0) - angle)
      if (shortfall > 1e-6) {
        spread[i] -= shortfall / 2
        spread[j] += shortfall / 2
        moved = true
      }
    })
    if (!moved) break
  }
  return spread
}

const at = (angle: number, radius: number): Point => ({
  x: CENTER.x + radius * Math.cos(angle),
  y: CENTER.y + radius * Math.sin(angle)
})

export function layoutTopology(topology: Topology): Layout {
  const relayIds = topology.relays.map((relay) => relay.id).sort()
  const clientsByRelay = new Map<string, ClientNode[]>(relayIds.map((id) => [id, []]))
  for (const client of topology.clients.values()) clientsByRelay.get(client.relayId)?.push(client)
  for (const clients of clientsByRelay.values()) {
    clients.sort((a, b) => a.appId.localeCompare(b.appId) || a.label.localeCompare(b.label))
  }

  const clientCount = topology.clients.size
  const minimumWeight = Math.max(1, clientCount / (2 * Math.max(1, relayIds.length)))
  const weights = relayIds.map((id) => Math.max(clientsByRelay.get(id)!.length, minimumWeight))
  const totalWeight = weights.reduce((a, b) => a + b, 0)
  const rings = ringsFor(clientCount)

  const relays = new Map<string, Point>()
  const clients = new Map<string, Point>()
  const relayAngles: number[] = []
  let arcStart = START_ANGLE
  let slot = 0
  relayIds.forEach((relayId, relayIndex) => {
    const arc = (weights[relayIndex] / totalWeight) * 2 * Math.PI
    const relayClients = clientsByRelay.get(relayId)!
    relayAngles.push(arcStart + arc / 2)
    relayClients.forEach((client, clientIndex) => {
      const angle = arcStart + ((clientIndex + 0.5) / relayClients.length) * arc
      clients.set(client.key, at(angle, FIRST_RING_RADIUS + (slot % rings) * RING_SPACING))
      slot += 1
    })
    arcStart += arc
  })

  if (relayIds.length === 1) {
    relays.set(relayIds[0], CENTER)
  } else if (relayIds.length > 1) {
    const orbit = Math.max(RELAY_ORBIT, RELAY_CLEARANCE / (2 * Math.sin(Math.PI / relayIds.length)))
    const gap = 2 * Math.asin(RELAY_CLEARANCE / (2 * orbit))
    spreadAngles(relayAngles, gap).forEach((angle, i) => relays.set(relayIds[i], at(angle, orbit)))
  }
  return { relays, clients }
}
