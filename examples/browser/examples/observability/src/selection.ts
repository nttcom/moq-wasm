import { type Route, type Topology, relativeNamespace } from './topology'

export type Selection = { kind: 'relay' | 'client' | 'link' | 'mesh'; id: string } | null

export const MESH: Selection = { kind: 'mesh', id: 'mesh' }

/** Every route and client is of the app the page's token names, so only the namespace narrows them. */
export interface Filters {
  namespacePrefix: string
}

const elements = (namespace: string) => namespace.split('/').filter(Boolean)

export function namespaceMatches(relative: string, prefix: string): boolean {
  const wanted = elements(prefix)
  return elements(relative).slice(0, wanted.length).join('/') === wanted.join('/')
}

function routeMatches(route: Route, filters: Filters): boolean {
  return namespaceMatches(relativeNamespace(route.namespace), filters.namespacePrefix)
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
    const publishesMatching = client.published.some((namespace) => namespaceMatches(namespace, filters.namespacePrefix))
    if (!filters.namespacePrefix || involved.has(client.key) || publishesMatching) clients.add(client.key)
  }
  const filtered = Boolean(filters.namespacePrefix)
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
  return {
    namespacePrefix: routes.length
      ? commonPrefix(routes.map((route) => relativeNamespace(route.namespace)))
      : current.namespacePrefix
  }
}

export function widenedFilters(current: Filters): Filters {
  return { namespacePrefix: elements(current.namespacePrefix).slice(0, -1).join('/') }
}
