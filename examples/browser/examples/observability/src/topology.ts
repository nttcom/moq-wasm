import type { Id, RelaySnapshot, SessionStats, SubscriptionStats, TrackStats } from './api'

const MAX_RELAY_HOPS = 4
const BITS_PER_BYTE = 8
const MEGA = 1_000_000

export type LinkKind = 'uplink' | 'downlink' | 'inter-relay'

export interface RelayNode {
  id: string
  snapshot: RelaySnapshot
}

export interface ClientNode {
  key: string
  relayId: string
  session: SessionStats
  label: string
  appId: string
  published: string[]
  subscribed: string[]
  trackCount: number
}

export interface Route {
  key: string
  trackKey: string
  appId: string
  namespace: string
  name: string
  publisher: string
  subscriber: string
  subscriberRelayId: string
  subscription: SubscriptionStats
  hops: string[]
}

export interface MeasuredSession {
  relayId: string
  session: SessionStats
  bySender: boolean
}

export interface Link {
  key: string
  from: string
  to: string
  kind: LinkKind
  routes: string[]
  measured: MeasuredSession | null
  trackMbps: Map<string, number>
  lossPercent: number | null
}

export interface Topology {
  relays: RelayNode[]
  clients: Map<string, ClientNode>
  routes: Route[]
  links: Map<string, Link>
}

export const clientKey = (relayId: string, sessionId: Id) => `${relayId}:${sessionId}`
export const linkKey = (from: string, to: string) => `${from}>${to}`
export const trackKeyOf = (namespace: string, name: string) => `${namespace}/${name}`
export const appIdOf = (namespace: string) => namespace.split('/')[0] ?? ''
export const relativeNamespace = (namespace: string) => namespace.split('/').slice(1).join('/')

const normalizeIp = (ip: string | null) => (ip ? ip.replace(/^::ffff:/, '') : null)

function ipOf(socketAddress: string | null): string | null {
  if (!socketAddress) return null
  const host = socketAddress.startsWith('[')
    ? socketAddress.slice(1, socketAddress.indexOf(']'))
    : socketAddress.slice(0, socketAddress.lastIndexOf(':'))
  return normalizeIp(host)
}

class RateSource {
  private readonly previous: Map<string, RelaySnapshot>

  constructor(previous: Map<string, RelaySnapshot>) {
    this.previous = previous
  }

  private elapsedSeconds(snapshot: RelaySnapshot): number | null {
    const previous = this.previous.get(snapshot.relay_id)
    if (!previous || previous.timestamp_ms >= snapshot.timestamp_ms) return null
    return (snapshot.timestamp_ms - previous.timestamp_ms) / 1000
  }

  private perSecond(snapshot: RelaySnapshot, current: number, previous: number | undefined): number {
    const elapsed = this.elapsedSeconds(snapshot)
    if (elapsed === null || previous === undefined || current < previous) return 0
    return (current - previous) / elapsed
  }

  previousSession(snapshot: RelaySnapshot, sessionId: Id): SessionStats | undefined {
    return this.previous.get(snapshot.relay_id)?.sessions.find((session) => session.session_id === sessionId)
  }

  trackMbps(snapshot: RelaySnapshot, track: TrackStats): number {
    const previous = this.previous
      .get(snapshot.relay_id)
      ?.tracks.find(
        (candidate) =>
          candidate.publisher_session_id === track.publisher_session_id &&
          candidate.namespace === track.namespace &&
          candidate.name === track.name
      )
    return (this.perSecond(snapshot, track.bytes_received, previous?.bytes_received) * BITS_PER_BYTE) / MEGA
  }

  subscriptionMbps(snapshot: RelaySnapshot, subscription: SubscriptionStats): number {
    const previous = this.previous
      .get(snapshot.relay_id)
      ?.subscriptions.find(
        (candidate) =>
          candidate.subscriber_session_id === subscription.subscriber_session_id &&
          candidate.request_id === subscription.request_id
      )
    return (this.perSecond(snapshot, subscription.bytes_sent, previous?.bytes_sent) * BITS_PER_BYTE) / MEGA
  }

  lossPercent(snapshot: RelaySnapshot, session: SessionStats): number | null {
    const previous = this.previousSession(snapshot, session.session_id)
    if (!previous) return null
    const sent = session.sent_packets - previous.sent_packets
    const lost = session.lost_packets - previous.lost_packets
    if (sent <= 0 || lost < 0) return 0
    return (lost / sent) * 100
  }
}

const isLoopback = (ip: string) => ip === '127.0.0.1' || ip === '::1'

class RelayIndex {
  readonly byId: Map<string, RelaySnapshot>
  private readonly addressesOf = new Map<string, Set<string>>()

  constructor(snapshots: RelaySnapshot[]) {
    this.byId = new Map(snapshots.map((snapshot) => [snapshot.relay_id, snapshot]))
    for (const snapshot of snapshots) {
      for (const session of snapshot.sessions) {
        const local = normalizeIp(session.local_ip)
        if (local && !isLoopback(local)) this.addressOf(snapshot.relay_id).add(local)
        const dialed = ipOf(session.remote_address)
        if (session.dialed_relay_id && dialed) this.addressOf(session.dialed_relay_id).add(dialed)
      }
    }
  }

  private addressOf(relayId: string): Set<string> {
    let addresses = this.addressesOf.get(relayId)
    if (!addresses) {
      addresses = new Set()
      this.addressesOf.set(relayId, addresses)
    }
    return addresses
  }

  session(relayId: string, sessionId: Id): SessionStats | undefined {
    return this.byId.get(relayId)?.sessions.find((session) => session.session_id === sessionId)
  }

  peerRelayOf(relayId: string, session: SessionStats): string | null {
    if (session.dialed_relay_id) return session.dialed_relay_id
    const remoteIp = ipOf(session.remote_address)
    if (!remoteIp) return null
    for (const [otherId, addresses] of this.addressesOf) {
      if (otherId !== relayId && addresses.has(remoteIp)) return otherId
    }
    return null
  }

  private sessionsTowards(relayId: string, peerRelayId: string): SessionStats[] {
    return (this.byId.get(relayId)?.sessions ?? []).filter(
      (session) => session.peer === 'relay' && this.peerRelayOf(relayId, session) === peerRelayId
    )
  }

  senderSessionTowards(relayId: string, peerRelayId: string): SessionStats | undefined {
    const candidates = this.sessionsTowards(relayId, peerRelayId)
    const subscribers = new Set(this.byId.get(relayId)?.subscriptions.map((sub) => sub.subscriber_session_id))
    return candidates.find((session) => subscribers.has(session.session_id)) ?? candidates[0]
  }

  receiverSessionTowards(relayId: string, peerRelayId: string): SessionStats | undefined {
    const candidates = this.sessionsTowards(relayId, peerRelayId)
    const publishers = new Set(this.byId.get(relayId)?.tracks.map((track) => track.publisher_session_id))
    return candidates.find((session) => publishers.has(session.session_id)) ?? candidates[0]
  }

  publisherSessionOf(relayId: string, namespace: string, name: string): Id | undefined {
    return this.byId.get(relayId)?.tracks.find((track) => track.namespace === namespace && track.name === name)
      ?.publisher_session_id
  }
}

export function isStatsSession(session: SessionStats): boolean {
  return session.peer === 'stats_publisher' || session.peer === 'stats_subscriber'
}

const clientLabel = (session: SessionStats) => session.remote_address ?? `session ${session.session_id}`

export function buildTopology(snapshots: RelaySnapshot[], previous: Map<string, RelaySnapshot>): Topology {
  const index = new RelayIndex(snapshots)
  const rates = new RateSource(previous)
  const clients = new Map<string, ClientNode>()
  for (const snapshot of snapshots) {
    for (const session of snapshot.sessions) {
      if (session.peer !== 'client') continue
      const key = clientKey(snapshot.relay_id, session.session_id)
      clients.set(key, {
        key,
        relayId: snapshot.relay_id,
        session,
        label: clientLabel(session),
        appId: session.app_id,
        published: [],
        subscribed: [],
        trackCount: 0
      })
    }
  }

  for (const snapshot of snapshots) {
    for (const track of snapshot.tracks) {
      const publisher = clients.get(clientKey(snapshot.relay_id, track.publisher_session_id))
      if (!publisher) continue
      publisher.trackCount += 1
      const namespace = relativeNamespace(track.namespace)
      if (!publisher.published.includes(namespace)) publisher.published.push(namespace)
    }
  }

  const routes: Route[] = []
  for (const snapshot of snapshots) {
    for (const subscription of snapshot.subscriptions) {
      const subscriber = clients.get(clientKey(snapshot.relay_id, subscription.subscriber_session_id))
      if (!subscriber) continue
      const hops = [linkKey(snapshot.relay_id, subscriber.key)]
      let relayId = snapshot.relay_id
      let publisherSessionId: Id | undefined = subscription.publisher_session_id
      let publisher: string | null = null
      for (let hop = 0; hop < MAX_RELAY_HOPS && publisherSessionId !== undefined; hop++) {
        const session = index.session(relayId, publisherSessionId)
        if (!session || isStatsSession(session)) break
        if (session.peer === 'client') {
          publisher = clientKey(relayId, publisherSessionId)
          hops.unshift(linkKey(publisher, relayId))
          break
        }
        const origin = index.peerRelayOf(relayId, session)
        if (!origin) break
        hops.unshift(linkKey(origin, relayId))
        relayId = origin
        publisherSessionId = index.publisherSessionOf(origin, subscription.namespace, subscription.name)
      }
      if (!publisher) continue
      const namespace = relativeNamespace(subscription.namespace)
      if (!subscriber.subscribed.includes(namespace)) subscriber.subscribed.push(namespace)
      routes.push({
        key: `${snapshot.relay_id}:${subscription.subscriber_session_id}:${subscription.request_id}`,
        trackKey: trackKeyOf(subscription.namespace, subscription.name),
        appId: appIdOf(subscription.namespace),
        namespace: subscription.namespace,
        name: subscription.name,
        publisher,
        subscriber: subscriber.key,
        subscriberRelayId: snapshot.relay_id,
        subscription,
        hops
      })
    }
  }

  const links = new Map<string, Link>()
  for (const route of routes) {
    for (const key of route.hops) {
      let link = links.get(key)
      if (!link) {
        const [from, to] = key.split('>')
        link = describeLink(from, to, clients, index, rates)
        links.set(key, link)
      }
      link.routes.push(route.key)
    }
  }

  return {
    relays: snapshots.map((snapshot) => ({ id: snapshot.relay_id, snapshot })),
    clients,
    routes,
    links
  }
}

function describeLink(
  from: string,
  to: string,
  clients: Map<string, ClientNode>,
  index: RelayIndex,
  rates: RateSource
): Link {
  const base = { key: linkKey(from, to), from, to, routes: [] as string[], trackMbps: new Map<string, number>() }
  const publisher = clients.get(from)
  if (publisher) {
    const snapshot = index.byId.get(to)!
    for (const track of snapshot.tracks) {
      if (track.publisher_session_id !== publisher.session.session_id) continue
      add(base.trackMbps, trackKeyOf(track.namespace, track.name), rates.trackMbps(snapshot, track))
    }
    return {
      ...base,
      kind: 'uplink',
      measured: { relayId: to, session: publisher.session, bySender: false },
      lossPercent: null
    }
  }
  const subscriber = clients.get(to)
  if (subscriber) {
    const snapshot = index.byId.get(from)!
    for (const subscription of snapshot.subscriptions) {
      if (subscription.subscriber_session_id !== subscriber.session.session_id) continue
      add(
        base.trackMbps,
        trackKeyOf(subscription.namespace, subscription.name),
        rates.subscriptionMbps(snapshot, subscription)
      )
    }
    return {
      ...base,
      kind: 'downlink',
      measured: { relayId: from, session: subscriber.session, bySender: true },
      lossPercent: rates.lossPercent(snapshot, subscriber.session)
    }
  }
  const sender = index.senderSessionTowards(from, to)
  const senderSnapshot = index.byId.get(from)
  if (sender && senderSnapshot) {
    for (const subscription of senderSnapshot.subscriptions) {
      if (subscription.subscriber_session_id !== sender.session_id) continue
      add(
        base.trackMbps,
        trackKeyOf(subscription.namespace, subscription.name),
        rates.subscriptionMbps(senderSnapshot, subscription)
      )
    }
    return {
      ...base,
      kind: 'inter-relay',
      measured: { relayId: from, session: sender, bySender: true },
      lossPercent: rates.lossPercent(senderSnapshot, sender)
    }
  }
  const receiver = index.receiverSessionTowards(to, from)
  const receiverSnapshot = index.byId.get(to)
  if (receiver && receiverSnapshot) {
    for (const track of receiverSnapshot.tracks) {
      if (track.publisher_session_id !== receiver.session_id) continue
      add(base.trackMbps, trackKeyOf(track.namespace, track.name), rates.trackMbps(receiverSnapshot, track))
    }
  }
  return {
    ...base,
    kind: 'inter-relay',
    measured: receiver ? { relayId: to, session: receiver, bySender: false } : null,
    lossPercent: null
  }
}

function add(rates: Map<string, number>, key: string, mbps: number) {
  rates.set(key, (rates.get(key) ?? 0) + mbps)
}

export function linkMbps(link: Link, visibleTrackKeys: Set<string> | null): number {
  let total = 0
  for (const [trackKey, mbps] of link.trackMbps) {
    if (!visibleTrackKeys || visibleTrackKeys.has(trackKey)) total += mbps
  }
  return total
}
