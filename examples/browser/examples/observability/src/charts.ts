import type { Id, SeriesTarget } from './api'
import type { Selection, Visibility } from './selection'
import { type Route, type Topology, relativeNamespace } from './topology'

export interface LineRef {
  label: string
  relayId: string
  target: SeriesTarget
  metric: string
}

export interface ChartSpec {
  title: string
  unit: string
  lines: LineRef[]
}

const UP = '↑'
const DOWN = '↓'

function chart(title: string, unit: string, lines: (LineRef | null | false)[]): ChartSpec | null {
  const present = lines.filter((line): line is LineRef => Boolean(line))
  return present.length ? { title, unit, lines: present } : null
}

const trackLabel = (namespace: string, name: string) => `${relativeNamespace(namespace)} / ${name}`

function subscriptionLines(routes: Route[], metric: string): LineRef[] {
  return routes.map((route) => ({
    label: trackLabel(route.namespace, route.name),
    relayId: route.subscriberRelayId,
    target: {
      target: 'subscription',
      subscriber_session_id: route.subscription.subscriber_session_id,
      request_id: route.subscription.request_id
    },
    metric
  }))
}

function trackLines(topology: Topology, relayId: string, publisherSessionId: Id, visible: Visibility, metric: string) {
  const snapshot = topology.relays.find((relay) => relay.id === relayId)?.snapshot
  return (snapshot?.tracks ?? [])
    .filter(
      (track) =>
        track.publisher_session_id === publisherSessionId &&
        (!visible.trackKeys || visible.trackKeys.has(`${track.namespace}/${track.name}`))
    )
    .map(
      (track): LineRef => ({
        label: trackLabel(track.namespace, track.name),
        relayId,
        target: {
          target: 'track',
          publisher_session_id: publisherSessionId,
          namespace: track.namespace,
          name: track.name
        },
        metric
      })
    )
}

function relayCharts(relayId: string): ChartSpec[] {
  const process: SeriesTarget = { target: 'process' }
  const relay: SeriesTarget = { target: 'relay' }
  const line = (label: string, target: SeriesTarget, metric: string): LineRef => ({ label, relayId, target, metric })
  return [
    chart('Memory', 'MB', [line('RSS', process, 'rss_mb'), line('Cache payload', process, 'cache_mb')]),
    chart('Throughput', 'Mbps', [line('Ingress', relay, 'ingress_mbps'), line('Egress', relay, 'egress_mbps')]),
    chart('Egress loss (bitrate-weighted)', '%', [line('Loss', relay, 'egress_loss_percent')]),
    chart('Sessions', '', [line('Sessions', relay, 'sessions')]),
    chart('Cache objects', '', [line('Objects', process, 'cache_objects')]),
    chart('Flow-control blocked', '/s', [
      line('Peers by relay window', relay, 'peers_blocked_per_s'),
      line('Relay by peer windows', relay, 'relay_blocked_per_s')
    ]),
    chart('Streams reset by peers', '/s', [line('Resets', relay, 'peer_resets_per_s')]),
    chart('Congestion events', '/s', [line('Events', relay, 'congestion_per_s')])
  ].filter((spec): spec is ChartSpec => spec !== null)
}

interface Direction {
  uplink: boolean
  downlink: boolean
  sessionBitrate: boolean
  peer: 'client' | 'peer relay'
}

function sessionCharts(
  topology: Topology,
  relayId: string,
  sessionId: Id,
  visible: Visibility,
  received: Route[],
  direction: Direction
): ChartSpec[] {
  const session: SeriesTarget = { target: 'session', session_id: sessionId }
  const line = (label: string, metric: string): LineRef => ({ label, relayId, target: session, metric })
  const { uplink, downlink, sessionBitrate, peer } = direction
  return [
    sessionBitrate
      ? chart('Bitrate', 'Mbps', [
          uplink && line(`${UP} received by relay`, 'received_mbps'),
          downlink && line(`${DOWN} sent by relay`, 'sent_mbps')
        ])
      : null,
    chart('RTT', 'ms', [line('RTT', 'rtt_ms')]),
    downlink ? chart('Downlink loss', '%', [line('Loss', 'loss_percent')]) : null,
    downlink ? chart('cwnd', 'KB', [line('cwnd', 'cwnd_kb')]) : null,
    downlink ? chart('Congestion events', '/s', [line('Events', 'congestion_per_s')]) : null,
    uplink
      ? chart('Max object arrival gap', 'ms', trackLines(topology, relayId, sessionId, visible, 'max_arrival_gap_ms'))
      : null,
    chart('Flow-control blocked', '/s', [
      uplink && line(`${UP} ${peer} by relay window`, 'peer_blocked_per_s'),
      downlink && line(`${DOWN} relay by ${peer} window`, 'relay_blocked_per_s')
    ]),
    chart(`Stream signals from ${peer}`, '/s', [
      uplink && line(`${UP} RESET_STREAM`, 'peer_resets_per_s'),
      downlink && line(`${DOWN} STOP_SENDING`, 'stop_sending_per_s')
    ]),
    downlink ? chart('Streams reset by relay', '/s', subscriptionLines(received, 'resets_per_s')) : null,
    downlink ? chart('Delivery lag', 'ms', subscriptionLines(received, 'lag_ms')) : null
  ].filter((spec): spec is ChartSpec => spec !== null)
}

export function chartSpecs(topology: Topology, selection: Selection, visible: Visibility): ChartSpec[] {
  if (!selection) return []
  if (selection.kind === 'relay') return relayCharts(selection.id)
  if (selection.kind === 'client') {
    const client = topology.clients.get(selection.id)
    if (!client) return []
    const received = visible.routes.filter((route) => route.subscriber === client.key)
    return sessionCharts(topology, client.relayId, client.session.session_id, visible, received, {
      uplink: client.trackCount > 0,
      downlink: received.length > 0,
      sessionBitrate: true,
      peer: 'client'
    })
  }
  const link = topology.links.get(selection.id)
  if (!link?.measured) return []
  const { relayId, session, bySender } = link.measured
  const carried = visible.routes.filter((route) => link.routes.includes(route.key))
  if (link.kind === 'uplink') {
    return [
      chart('Bitrate', 'Mbps', trackLines(topology, relayId, session.session_id, visible, 'received_mbps')),
      ...sessionCharts(topology, relayId, session.session_id, visible, [], {
        uplink: true,
        downlink: false,
        sessionBitrate: false,
        peer: 'client'
      })
    ].filter((spec): spec is ChartSpec => spec !== null)
  }
  if (link.kind === 'downlink') {
    return [
      chart('Bitrate', 'Mbps', subscriptionLines(carried, 'sent_mbps')),
      ...sessionCharts(topology, relayId, session.session_id, visible, carried, {
        uplink: false,
        downlink: true,
        sessionBitrate: false,
        peer: 'client'
      })
    ].filter((spec): spec is ChartSpec => spec !== null)
  }
  const sender = topology.relays.find((relay) => relay.id === relayId)?.snapshot
  const carriedTracks = new Set(carried.map((route) => route.trackKey))
  const relayedSubscriptions = (sender?.subscriptions ?? [])
    .filter(
      (subscription) =>
        subscription.subscriber_session_id === session.session_id &&
        carriedTracks.has(`${subscription.namespace}/${subscription.name}`)
    )
    .map(
      (subscription): LineRef => ({
        label: trackLabel(subscription.namespace, subscription.name),
        relayId,
        target: {
          target: 'subscription',
          subscriber_session_id: subscription.subscriber_session_id,
          request_id: subscription.request_id
        },
        metric: 'sent_mbps'
      })
    )
  return [
    bySender ? chart('Bitrate', 'Mbps', relayedSubscriptions) : null,
    ...sessionCharts(topology, relayId, session.session_id, visible, [], {
      uplink: !bySender,
      downlink: bySender,
      sessionBitrate: !bySender,
      peer: 'peer relay'
    })
  ].filter((spec): spec is ChartSpec => spec !== null)
}
