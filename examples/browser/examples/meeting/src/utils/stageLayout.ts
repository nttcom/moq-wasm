export type StageView = 'gallery' | 'speaker'

/// One picture on the stage: a member's camera or screen share. The local
/// member is `local`.
export type StageTile = { key: string; memberId: string; source: 'camera' | 'screenshare' }

export type StageInput = {
  view: StageView
  filmstripOpen: boolean
  remoteMemberIds: string[]
  /// Remote members whose catalog lists a screen share.
  remoteSharingIds: Set<string>
  localSharing: boolean
  activeSpeakerId: string | undefined
}

export type StageArrangement = {
  view: StageView
  focus: StageTile | undefined
  /// The gallery grid, or the filmstrip next to the focus.
  others: StageTile[]
  visibleKeys: Set<string>
}

export const LOCAL_MEMBER_ID = 'local'

export function tileKey(memberId: string, source: StageTile['source']): string {
  return `${memberId}:${source}`
}

function tile(memberId: string, source: StageTile['source']): StageTile {
  return { key: tileKey(memberId, source), memberId, source }
}

/// A screen share takes the focus, a remote one before the local one, and
/// turns the stage to the speaker view; otherwise the speaker view focuses the
/// active speaker, or the first remote member until someone speaks. The
/// filmstrip holds every other picture and shows none of them while closed.
export function arrangeStage(input: StageInput): StageArrangement {
  const cameras = [tile(LOCAL_MEMBER_ID, 'camera'), ...input.remoteMemberIds.map((id) => tile(id, 'camera'))]
  const shares = [
    ...input.remoteMemberIds.filter((id) => input.remoteSharingIds.has(id)).map((id) => tile(id, 'screenshare')),
    ...(input.localSharing ? [tile(LOCAL_MEMBER_ID, 'screenshare')] : [])
  ]
  const view: StageView = shares.length > 0 ? 'speaker' : input.view
  if (view === 'gallery') {
    return { view, focus: undefined, others: cameras, visibleKeys: new Set(cameras.map((camera) => camera.key)) }
  }
  const speaker =
    input.activeSpeakerId && input.remoteMemberIds.includes(input.activeSpeakerId)
      ? input.activeSpeakerId
      : input.remoteMemberIds[0]
  const focus = shares[0] ?? tile(speaker ?? LOCAL_MEMBER_ID, 'camera')
  const others = [...shares, ...cameras].filter((candidate) => candidate.key !== focus.key)
  const visible = [focus, ...(input.filmstripOpen ? others : [])]
  return { view, focus, others, visibleKeys: new Set(visible.map((candidate) => candidate.key)) }
}

const TILE_ASPECT = 16 / 9

/// The width of the largest 16:9 tiles that fit `count` of them, `gap` apart,
/// into a `width` × `height` box, over every column count.
export function galleryTileWidth(count: number, width: number, height: number, gap: number): number {
  let best = 0
  for (let columns = 1; columns <= Math.max(1, count); columns += 1) {
    const rows = Math.ceil(count / columns)
    const byWidth = (width - gap * (columns - 1)) / columns
    const byHeight = ((height - gap * (rows - 1)) / rows) * TILE_ASPECT
    best = Math.max(best, Math.min(byWidth, byHeight))
  }
  return Math.max(0, Math.floor(best))
}
