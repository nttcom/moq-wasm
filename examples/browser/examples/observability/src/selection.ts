import { type Route, type Topology, relativeNamespace } from './topology'

export type Selection = { kind: 'relay' | 'client' | 'link' | 'mesh'; id: string } | null

export const MESH: Selection = { kind: 'mesh', id: 'mesh' }

export interface Filters {
  appId: string
  namespacePrefix: string
}

const elements = (namespace: string) => namespace.split('/').filter(Boolean)

export function namespaceMatches(relative: string, prefix: string): boolean {
  const wanted = elements(prefix)
  return elements(relative).slice(0, wanted.length).join('/') === wanted.join('/')
}

function routeMatches(route: Route, filters: Filters): boolean {
  return (
    (!filters.appId || route.appId === filters.appId) &&
    namespaceMatches(relativeNamespace(route.namespace), filters.namespacePrefix)
  )
}

export interface Visibility {
  routes: Route[]
  routeKeys: Set<string>
  trackKeys: Set<string> | null
  clients: Set<string>
  links: Set<string>
}

export function visibility(topology: Topology, filters: Filters): Visibility {
  const routes = topology.routes.filter((route) => routeMatches(route, filters))
  const involved = new Set(routes.flatMap((route) => [route.publisher, route.subscriber]))
  const clients = new Set<string>()
  for (const client of topology.clients.values()) {
    if (filters.appId && client.appId !== filters.appId) continue
    const publishesMatching = client.published.some((namespace) => namespaceMatches(namespace, filters.namespacePrefix))
    if (!filters.namespacePrefix || involved.has(client.key) || publishesMatching) clients.add(client.key)
  }
  const filtered = Boolean(filters.appId || filters.namespacePrefix)
  return {
    routes,
    routeKeys: new Set(routes.map((route) => route.key)),
    trackKeys: filtered ? new Set(routes.map((route) => route.trackKey)) : null,
    clients,
    links: new Set(routes.flatMap((route) => route.hops))
  }
}

export function routesOf(topology: Topology, selection: Selection, visible: Visibility): Route[] {
  if (!selection) return []
  if (selection.kind === 'mesh') return visible.routes
  if (selection.kind === 'link') {
    const link = topology.links.get(selection.id)
    return visible.routes.filter((route) => link?.routes.includes(route.key))
  }
  return visible.routes.filter(
    (route) =>
      route.publisher === selection.id ||
      route.subscriber === selection.id ||
      route.hops.some((hop) => hop.split('>').includes(selection.id))
  )
}

function commonPrefix(namespaces: string[]): string {
  if (namespaces.length === 0) return ''
  return namespaces
    .map(elements)
    .reduce((common, current) => {
      let length = 0
      while (length < common.length && common[length] === current[length]) length += 1
      return common.slice(0, length)
    })
    .join('/')
}

export function narrowedFilters(routes: Route[], current: Filters): Filters {
  const apps = [...new Set(routes.map((route) => route.appId))]
  return {
    appId: apps.length === 1 ? apps[0] : current.appId,
    namespacePrefix: routes.length
      ? commonPrefix(routes.map((route) => relativeNamespace(route.namespace)))
      : current.namespacePrefix
  }
}

export function widenedFilters(current: Filters, chosenAppId: string): Filters {
  const prefix = elements(current.namespacePrefix)
  if (prefix.length) return { ...current, namespacePrefix: prefix.slice(0, -1).join('/') }
  return { namespacePrefix: '', appId: current.appId === chosenAppId ? '' : chosenAppId }
}
