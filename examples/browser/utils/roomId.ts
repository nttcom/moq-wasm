// One namespace element: the room becomes an element of the page's namespaces.
const ROOM_ID_PATTERN = /^[a-z0-9-]{1,32}$/

export function roomIdFromSearch(search: string): string | undefined {
  const roomId = new URLSearchParams(search).get('room')?.trim()
  return roomId && ROOM_ID_PATTERN.test(roomId) ? roomId : undefined
}

/// The `room` query parameter, or a new room ID written into the URL so the link can be shared.
export function pageRoomId(): string {
  const url = new URL(window.location.href)
  const given = roomIdFromSearch(url.search)
  if (given) {
    return given
  }
  const created = crypto.randomUUID().slice(0, 8)
  url.searchParams.set('room', created)
  window.history.replaceState(window.history.state, '', url)
  return created
}
