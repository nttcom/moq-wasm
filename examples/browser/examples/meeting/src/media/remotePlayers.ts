import type { MoqSession } from 'moq-wasm'
import {
  type AudioLevel,
  type LivePlayer,
  type LivePlayerStats,
  type MediaCatalogTrack,
  RemoteBroadcast,
  type SourceView
} from 'moq-wasm/media'
import { DEFAULT_PLAYOUT_SETTINGS, type PlayoutSettings } from '../types/playout'
import { SCREEN_SHARE_SOURCE } from '../utils/catalogTrackName'

export type RemoteVideoSource = 'camera' | 'screenshare'
export const REMOTE_VIDEO_SOURCES: RemoteVideoSource[] = ['camera', 'screenshare']
export type RemotePlaybackStats = Partial<Record<RemoteVideoSource, LivePlayerStats>>

/// The source names a member publishes each tile under. `video` is the camera
/// of a member whose page still publishes with `publish()`.
const SOURCE_NAMES: Record<RemoteVideoSource, string[]> = {
  camera: ['camera', 'video'],
  screenshare: [SCREEN_SHARE_SOURCE]
}

export type CatalogState = 'loading' | 'loaded' | 'absent'

export type RemoteSourceView = {
  videoTracks: MediaCatalogTrack[]
  selectedVideoTrack: string
  adaptiveVideo: boolean
  videoSubscribed: boolean
  /// A frame of the subscribed video has been shown.
  showsPicture: boolean
}

export type RemoteMemberView = {
  catalog: CatalogState
  camera: RemoteSourceView
  screenshare: RemoteSourceView
  audioTracks: MediaCatalogTrack[]
  selectedAudioTrack: string
  audioSubscribed: boolean
}

export type RemotePlayersHandlers = {
  /// A member's view changed.
  onChange?: () => void
  onAudioLevel?: (memberId: string, audioLevel: AudioLevel) => void
}

type MemberPlayers = {
  broadcast: RemoteBroadcast
  catalogState: CatalogState
  unlisten: () => void
  /// Applied to a source's view once the catalog lists it.
  shown: Record<RemoteVideoSource, boolean>
  /// The view the handlers were last told about, to skip the changes a page does not show.
  viewKey: string
  removed: boolean
}

const EMPTY_SOURCE_VIEW: RemoteSourceView = {
  videoTracks: [],
  selectedVideoTrack: '',
  adaptiveVideo: true,
  videoSubscribed: false,
  showsPicture: false
}

/// One `RemoteBroadcast` per member on the session. Its camera view also
/// plays the member's audio. A source's video is subscribed only while its
/// tile is shown.
export class RemotePlayers {
  private session: MoqSession | undefined
  private handlers: RemotePlayersHandlers = {}
  private readonly members = new Map<string, MemberPlayers>()
  private readonly playoutSettings = new Map<string, PlayoutSettings>()

  connect(session: MoqSession): void {
    this.session = session
  }

  setHandlers(handlers: RemotePlayersHandlers): void {
    this.handlers = handlers
  }

  /// Plays the member published at `trackNamespace`. A member announced again
  /// (a rejoin) starts over on the new publisher.
  add(memberId: string, trackNamespace: string[]): void {
    const session = this.session
    if (!session) {
      return
    }
    this.remove(memberId)
    const broadcast = new RemoteBroadcast(session, trackNamespace, {
      onLog: (level, message) => {
        if (level !== 'info') {
          console[level](`[meeting][${memberId}] ${message}`)
        }
      }
    })
    const member: MemberPlayers = {
      broadcast,
      catalogState: 'loading',
      unlisten: () => {},
      shown: { camera: false, screenshare: false },
      viewKey: '',
      removed: false
    }
    this.members.set(memberId, member)
    const unlistenCatalog = broadcast.catalog.listen({
      onCatalog: () => this.setCatalogState(member, 'loaded'),
      onPublisherAbsent: () => this.setCatalogState(member, 'absent')
    })
    const unlistenSources = broadcast.listen({
      onSourceAdd: (view) => this.sourceAdded(memberId, member, view),
      onSourceRemove: () => this.notify(member),
      onAudioLevel: (view, audioLevel) => {
        if (sourceOf(view) === 'camera') {
          this.handlers.onAudioLevel?.(memberId, audioLevel)
        }
      }
    })
    member.unlisten = () => {
      unlistenCatalog()
      unlistenSources()
    }
    this.notify(member)
  }

  remove(memberId: string): void {
    const member = this.members.get(memberId)
    if (!member) {
      return
    }
    this.members.delete(memberId)
    member.removed = true
    member.unlisten()
    member.broadcast
      .dispose()
      .catch((error: unknown) => console.warn(`[meeting] failed to release the players of ${memberId}`, error))
    this.handlers.onChange?.()
  }

  /// The element a source's player draws into, for its tile to place;
  /// undefined while the member's catalog lists no such source.
  container(memberId: string, source: RemoteVideoSource): HTMLElement | undefined {
    return this.sourceView(memberId, source)?.element
  }

  /// Subscribes the source's video while its tile is shown, so the publisher
  /// can stop sending a hidden one; the camera's audio plays either way.
  setShown(memberId: string, source: RemoteVideoSource, shown: boolean): void {
    const member = this.members.get(memberId)
    if (!member) {
      return
    }
    member.shown[source] = shown
    this.sourceView(memberId, source)
      ?.setVideoEnabled(shown)
      .catch((error: unknown) => console.error(`[meeting] ${memberId} ${source} video ${shown ? 'on' : 'off'}`, error))
  }

  /// `trackName` undefined leaves the video rendition to the player's automatic choice.
  selectVideoTrack(memberId: string, source: RemoteVideoSource, trackName: string | undefined): void {
    const player = this.sourceView(memberId, source)?.player
    if (!player) {
      return
    }
    if (trackName === undefined) {
      player.setAdaptiveVideo(true)
      return
    }
    player
      .selectVideoTrack(trackName)
      .catch((error: unknown) => console.error(`[meeting] ${memberId} ${source} track ${trackName}`, error))
  }

  selectAudioTrack(memberId: string, trackName: string): void {
    this.sourceView(memberId, 'camera')
      ?.player.selectAudioTrack(trackName)
      .catch((error: unknown) => console.error(`[meeting] ${memberId} audio track ${trackName}`, error))
  }

  setPlayoutSettings(memberId: string, settings: PlayoutSettings): void {
    this.playoutSettings.set(memberId, settings)
    for (const view of this.members.get(memberId)?.broadcast.sources.values() ?? []) {
      applyPlayoutSettings(view.player, settings)
    }
  }

  view(memberId: string): RemoteMemberView | undefined {
    const member = this.members.get(memberId)
    return member && memberView(member)
  }

  stats(memberId: string): RemotePlaybackStats {
    return Object.fromEntries(
      REMOTE_VIDEO_SOURCES.flatMap((source) => {
        const view = this.sourceView(memberId, source)
        return view && (view.player.state.videoTracks.length > 0 || view.player.state.audioTracks.length > 0)
          ? [[source, view.stats()]]
          : []
      })
    )
  }

  dispose(): void {
    for (const memberId of [...this.members.keys()]) {
      this.remove(memberId)
    }
    this.playoutSettings.clear()
    this.handlers = {}
    this.session = undefined
  }

  private sourceView(memberId: string, source: RemoteVideoSource): SourceView | undefined {
    const member = this.members.get(memberId)
    return member && viewOf(member.broadcast, source)
  }

  private sourceAdded(memberId: string, member: MemberPlayers, view: SourceView): void {
    const source = sourceOf(view)
    if (!source) {
      return
    }
    applyPlayoutSettings(view.player, this.playoutSettings.get(memberId) ?? DEFAULT_PLAYOUT_SETTINGS)
    void view.setVideoEnabled(member.shown[source])
    view.listen({ onChange: () => this.notify(member), onPlayerChange: () => this.notify(member) })
    this.notify(member)
  }

  private setCatalogState(member: MemberPlayers, state: CatalogState): void {
    member.catalogState = state
    this.notify(member)
  }

  /// Calls `onChange` only when the view changes: each player reports its
  /// playback position several times a second, which the view does not show.
  private notify(member: MemberPlayers): void {
    if (member.removed) {
      return
    }
    const key = JSON.stringify(memberView(member))
    if (key !== member.viewKey) {
      member.viewKey = key
      this.handlers.onChange?.()
    }
  }
}

function sourceOf(view: SourceView): RemoteVideoSource | undefined {
  return REMOTE_VIDEO_SOURCES.find((source) => SOURCE_NAMES[source].includes(view.name))
}

function viewOf(broadcast: RemoteBroadcast, source: RemoteVideoSource): SourceView | undefined {
  return [...broadcast.sources.values()].find((view) => sourceOf(view) === source)
}

function memberView({ catalogState, broadcast }: MemberPlayers): RemoteMemberView {
  const camera = viewOf(broadcast, 'camera')?.player.state
  return {
    catalog: catalogState,
    camera: sourceView(viewOf(broadcast, 'camera')),
    screenshare: sourceView(viewOf(broadcast, 'screenshare')),
    audioTracks: camera?.audioTracks ?? [],
    selectedAudioTrack: camera?.selectedAudioTrack ?? '',
    audioSubscribed: camera?.delivery.liveAudioAlias !== undefined
  }
}

function sourceView(view: SourceView | undefined): RemoteSourceView {
  if (!view) {
    return EMPTY_SOURCE_VIEW
  }
  const { videoTracks, selectedVideoTrack, adaptiveVideo, delivery } = view.player.state
  const videoSubscribed = delivery.liveVideoAlias !== undefined
  return {
    videoTracks,
    selectedVideoTrack,
    adaptiveVideo,
    videoSubscribed,
    showsPicture: videoSubscribed && view.state.video === 'playing'
  }
}

function applyPlayoutSettings(player: LivePlayer, settings: PlayoutSettings): void {
  player.setBufferPolicy(settings.policy)
  player.setCatchUp(settings.catchUp)
}
