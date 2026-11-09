import type { MoqSession } from 'moq-wasm'
import {
  type AudioLevel,
  BroadcastCatalog,
  type LivePlayerStats,
  type LiveSource,
  LivePlayer,
  type MediaCatalogTrack
} from 'moq-wasm/media'
import { DEFAULT_PLAYOUT_SETTINGS, type PlayoutSettings } from '../types/playout'
import { SCREEN_SHARE_SOURCE } from '../utils/catalogTrackName'

export type RemoteVideoSource = 'camera' | 'screenshare'
export const REMOTE_VIDEO_SOURCES: RemoteVideoSource[] = ['camera', 'screenshare']
export type RemotePlaybackStats = Partial<Record<RemoteVideoSource, LivePlayerStats>>

/// The source names the publisher passes to `publish()`.
const SOURCES: Record<RemoteVideoSource, LiveSource> = {
  camera: { video: 'video', audio: 'audio' },
  screenshare: { video: SCREEN_SHARE_SOURCE }
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

type SourcePlayer = { player: LivePlayer; container: HTMLDivElement; showsPicture: boolean }

type MemberPlayers = {
  catalog: BroadcastCatalog
  catalogState: CatalogState
  unlistenCatalog: () => void
  sources: Record<RemoteVideoSource, SourcePlayer>
  /// The view the handlers were last told about, to skip the changes a page does not show.
  viewKey: string
}

/// One `LivePlayer` per member and source on the session, sharing the
/// member's `BroadcastCatalog`. The camera's player also plays the member's
/// audio. A source's video is subscribed only while its tile is shown.
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
    const catalog = new BroadcastCatalog(session, trackNamespace)
    const member: MemberPlayers = {
      catalog,
      catalogState: 'loading',
      unlistenCatalog: () => {},
      sources: {
        camera: this.createPlayer(memberId, 'camera'),
        screenshare: this.createPlayer(memberId, 'screenshare')
      },
      viewKey: ''
    }
    this.members.set(memberId, member)
    member.unlistenCatalog = catalog.listen({
      onCatalog: () => this.setCatalogState(member, 'loaded'),
      onPublisherAbsent: () => this.setCatalogState(member, 'absent')
    })
    for (const source of REMOTE_VIDEO_SOURCES) {
      member.sources[source].player
        .start(session, trackNamespace, catalog)
        .catch((error: unknown) => console.error(`[meeting] ${memberId} ${source} player failed to start`, error))
    }
    this.notify(member)
  }

  remove(memberId: string): void {
    const member = this.members.get(memberId)
    if (!member) {
      return
    }
    this.members.delete(memberId)
    member.unlistenCatalog()
    void (async () => {
      for (const { player } of Object.values(member.sources)) {
        await player.dispose()
      }
      await member.catalog.close()
    })().catch((error: unknown) => console.warn(`[meeting] failed to release the players of ${memberId}`, error))
    this.handlers.onChange?.()
  }

  /// The element a source's player draws into, for its tile to place.
  container(memberId: string, source: RemoteVideoSource): HTMLElement | undefined {
    return this.members.get(memberId)?.sources[source].container
  }

  /// Subscribes the source's video while its tile is shown, so the publisher
  /// can stop sending a hidden one; the camera's audio plays either way.
  setShown(memberId: string, source: RemoteVideoSource, shown: boolean): void {
    this.members
      .get(memberId)
      ?.sources[source].player.setVideoEnabled(shown)
      .catch((error: unknown) => console.error(`[meeting] ${memberId} ${source} video ${shown ? 'on' : 'off'}`, error))
  }

  /// `trackName` undefined leaves the video rendition to the player's automatic choice.
  selectVideoTrack(memberId: string, source: RemoteVideoSource, trackName: string | undefined): void {
    const player = this.members.get(memberId)?.sources[source].player
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
    this.members
      .get(memberId)
      ?.sources.camera.player.selectAudioTrack(trackName)
      .catch((error: unknown) => console.error(`[meeting] ${memberId} audio track ${trackName}`, error))
  }

  setPlayoutSettings(memberId: string, settings: PlayoutSettings): void {
    this.playoutSettings.set(memberId, settings)
    for (const { player } of Object.values(this.members.get(memberId)?.sources ?? {})) {
      applyPlayoutSettings(player, settings)
    }
  }

  view(memberId: string): RemoteMemberView | undefined {
    const member = this.members.get(memberId)
    return member && memberView(member)
  }

  stats(memberId: string): RemotePlaybackStats {
    const member = this.members.get(memberId)
    if (!member) {
      return {}
    }
    return Object.fromEntries(
      REMOTE_VIDEO_SOURCES.flatMap((source) => {
        const { player } = member.sources[source]
        return player.state.videoTracks.length > 0 || player.state.audioTracks.length > 0
          ? [[source, player.stats()]]
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

  private createPlayer(memberId: string, source: RemoteVideoSource): SourcePlayer {
    const container = document.createElement('div')
    container.className = 'meeting-player h-full w-full'
    // The callbacks read `player` and `sourcePlayer` only once the player has started.
    const player: LivePlayer = new LivePlayer({
      container,
      source: SOURCES[source],
      callbacks: {
        onStateChange: () => {
          if (player.state.delivery.liveVideoAlias === undefined) {
            sourcePlayer.showsPicture = false
          }
          this.notifyMember(memberId)
        },
        onLiveFrame: () => {
          if (!sourcePlayer.showsPicture) {
            sourcePlayer.showsPicture = true
            this.notifyMember(memberId)
          }
        },
        onLog: (level, message) => {
          if (level !== 'info') {
            console[level](`[meeting][${memberId}][${source}] ${message}`)
          }
        },
        onAudioLevel:
          source === 'camera' ? (audioLevel) => this.handlers.onAudioLevel?.(memberId, audioLevel) : undefined
      }
    })
    const sourcePlayer: SourcePlayer = { player, container, showsPicture: false }
    // Off until the tile is shown, so the publisher sends no video for a member behind a closed filmstrip at join.
    void player.setVideoEnabled(false)
    applyPlayoutSettings(player, this.playoutSettings.get(memberId) ?? DEFAULT_PLAYOUT_SETTINGS)
    return sourcePlayer
  }

  private setCatalogState(member: MemberPlayers, state: CatalogState): void {
    member.catalogState = state
    this.notify(member)
  }

  private notifyMember(memberId: string): void {
    const member = this.members.get(memberId)
    if (member) {
      this.notify(member)
    }
  }

  /// Calls `onChange` only when the view changes: each player reports its
  /// playback position several times a second, which the view does not show.
  private notify(member: MemberPlayers): void {
    const key = JSON.stringify(memberView(member))
    if (key !== member.viewKey) {
      member.viewKey = key
      this.handlers.onChange?.()
    }
  }
}

function memberView({ catalogState, sources }: MemberPlayers): RemoteMemberView {
  const camera = sources.camera.player.state
  return {
    catalog: catalogState,
    camera: sourceView(sources.camera),
    screenshare: sourceView(sources.screenshare),
    audioTracks: camera.audioTracks,
    selectedAudioTrack: camera.selectedAudioTrack,
    audioSubscribed: camera.delivery.liveAudioAlias !== undefined
  }
}

function sourceView({ player, showsPicture }: SourcePlayer): RemoteSourceView {
  const { videoTracks, selectedVideoTrack, adaptiveVideo, delivery } = player.state
  const videoSubscribed = delivery.liveVideoAlias !== undefined
  return {
    videoTracks,
    selectedVideoTrack,
    adaptiveVideo,
    videoSubscribed,
    showsPicture: videoSubscribed && showsPicture
  }
}

function applyPlayoutSettings(player: LivePlayer, settings: PlayoutSettings): void {
  player.setBufferPolicy(settings.policy)
  player.setCatchUp(settings.catchUp)
}
