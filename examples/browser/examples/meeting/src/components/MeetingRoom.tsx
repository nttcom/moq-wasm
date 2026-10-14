import { FormEvent, useCallback, useEffect, useMemo, useRef, useState } from 'react'
import { LocalSession } from '../session/localSession'
import { Room } from '../types/room'
import { ChatMessage } from '../types/chat'
import { RemoteMember } from '../types/member'
import { ChatSidebar } from './ChatSidebar'
import { MemberGrid, memberStates } from './MemberGrid'
import { RoomHeader } from './RoomHeader'
import { useSessionEventHandlers } from '../hooks/useSessionEventHandlers'
import { useMeetingMedia } from '../hooks/useMeetingMedia'
import { useRetryBackoff } from '../hooks/useRetryBackoff'
import {
  CATALOG_SUBSCRIBE_ROLES,
  type CatalogSelections,
  type MeetingCatalogTrack,
  type CatalogSubscribeRole,
  type TrackMediaConfig,
  AUTO_TRACK_SELECTION,
  isSwitchableRole
} from '../types/catalog'
import { catalogTracksForRole, pickHighestBitrateTrack } from '../media/meetingCatalog'
import { checkCodecSupport } from '../utils/codecSupport'
import type { RemotePlaybackStats, VideoAdaptation } from '../media/mediaSubscriber'
import type { SidebarStatsSample, VideoSendTiming } from '../types/stats'
import { buildTrackActionKey, updateMemberSubscriptionStateByRole } from '../utils/state/roomState'
import { type StageView, arrangeStage, tileKey } from '../utils/stageLayout'
import { capLadderToHeight } from '@player/adaptiveBitrate'

interface MeetingRoomProps {
  session: LocalSession
  onLeave: () => void
}

type StatsSnapshot = {
  localMemberId: string
  localVideoBitrate: number | null
  localAudioBitrate: number | null
  localCameraVideoSendTiming: VideoSendTiming | null
  localScreenShareVideoSendTiming: VideoSendTiming | null
  remoteMembers: RemoteMember[]
}

const SIDEBAR_STATS_SAMPLE_INTERVAL_MS = 1000
const SIDEBAR_STATS_HISTORY_LIMIT = 120
const AUTO_SUBSCRIBED_ROLES = ['audio', 'video', 'screenshare'] as const

export function MeetingRoom({ session, onLeave }: MeetingRoomProps) {
  const roomName = session.roomName
  const userName = session.localMember.name
  const [chatMessage, setChatMessage] = useState('')
  const [chatMessages, setChatMessages] = useState<ChatMessage[]>([])
  const [isSidebarOpen, setIsSidebarOpen] = useState(false)
  const [stageView, setStageView] = useState<StageView>('gallery')
  const [filmstripOpen, setFilmstripOpen] = useState(true)
  /// The shown height of every remote tile, in device pixels.
  const [tileHeights, setTileHeights] = useState<Map<string, number>>(new Map())
  const handleTileHeight = useCallback(
    (key: string, heightPx: number) =>
      setTileHeights((prev) => (prev.get(key) === heightPx ? prev : new Map(prev).set(key, heightPx))),
    []
  )
  const [statsHistory, setStatsHistory] = useState<Map<string, SidebarStatsSample[]>>(new Map())
  const [remoteStats, setRemoteStats] = useState<Map<string, RemotePlaybackStats>>(new Map())
  const [remoteCatalogTracks, setRemoteCatalogTracks] = useState<Map<string, MeetingCatalogTrack[]>>(new Map())
  const [remoteCatalogSelections, setRemoteCatalogSelections] = useState<Map<string, CatalogSelections>>(new Map())
  const [catalogLoadingMemberIds, setCatalogLoadingMemberIds] = useState<Set<string>>(new Set())
  const [catalogSubscribedMemberIds, setCatalogSubscribedMemberIds] = useState<Set<string>>(new Set())
  const [catalogUnsubscribingTrackKeys, setCatalogUnsubscribingTrackKeys] = useState<Set<string>>(new Set())
  /// Switchable roles follow their picture's adaptation unless a track was picked by hand.
  const [manualTrackKeys, setManualTrackKeys] = useState<Set<string>>(new Set())
  /// The `onPreferred` callback is fixed at SUBSCRIBE; this ref routes it to
  /// the latest render's handler.
  const adaptiveChoiceRef = useRef<(memberId: string, role: CatalogSubscribeRole, trackName: string) => void>(() => {})
  const { backoff: chatSubscribeBackoff, retryTick: chatSubscribeRetryTick } = useRetryBackoff('chat-auto-subscribe')
  const { backoff: catalogSubscribeBackoff, retryTick: catalogSubscribeRetryTick } =
    useRetryBackoff('catalog-auto-subscribe')
  const { backoff: mediaSubscribeBackoff, retryTick: mediaSubscribeRetryTick } = useRetryBackoff('media-auto-subscribe')
  const {
    activeSpeakerId,
    speakingMemberIds,
    cameraEnabled,
    screenShareEnabled,
    microphoneEnabled,
    cameraBusy,
    microphoneBusy,
    localVideoStream,
    localScreenShareStream,
    localAudioBitrate,
    localVideoBitrate,
    localCameraVideoSendTiming,
    localScreenShareVideoSendTiming,
    remotePictures,
    toggleCamera,
    toggleScreenShare,
    toggleMicrophone,
    playoutSettings,
    setPlayoutSettings,
    videoCodecOptions,
    videoResolutionOptions,
    videoBitrateOptions,
    videoHardwareAccelerationOptions,
    selectedVideoEncoding,
    selectVideoEncoding,
    selectedScreenShareEncoding,
    selectScreenShareEncoding,
    audioCodecOptions,
    audioBitrateOptions,
    audioChannelOptions,
    videoDevices,
    audioDevices,
    selectedVideoDeviceId,
    selectedAudioDeviceId,
    selectVideoDevice,
    selectAudioDevice,
    captureSettings,
    updateCaptureSettings,
    applyCaptureSettings,
    catalogTracks,
    subscribedCatalogTracks,
    addCatalogTrack,
    updateCatalogTrack,
    removeCatalogTrack
  } = useMeetingMedia(session)

  const statsSnapshotRef = useRef<StatsSnapshot>({
    localMemberId: session.localMember.id,
    localVideoBitrate,
    localAudioBitrate,
    localCameraVideoSendTiming,
    localScreenShareVideoSendTiming,
    remoteMembers: []
  })

  const [room, setRoom] = useState<Room>(() => ({
    name: roomName,
    localMember: session.localMember,
    remoteMembers: new Map()
  }))

  const handleSendChatMessage = async (event?: FormEvent) => {
    event?.preventDefault()
    const trimmed = chatMessage.trim()
    if (!trimmed) {
      return
    }
    try {
      await session.sendChatMessage(trimmed)
      const timestamp = Date.now()
      const namespace = [...session.trackNamespace]
      const localMessage: ChatMessage = {
        sender: userName,
        trackNamespace: namespace,
        groupId: BigInt(timestamp),
        text: trimmed,
        timestamp,
        isLocal: true
      }
      setChatMessages((prev) => [...prev, localMessage])
      setChatMessage('')
    } catch (error) {
      console.error('Failed to send chat message:', error)
    }
  }

  useEffect(() => {
    setRoom({
      name: session.roomName,
      localMember: session.localMember,
      remoteMembers: new Map()
    })
    setRemoteCatalogTracks(new Map())
    setRemoteCatalogSelections(new Map())
    setCatalogLoadingMemberIds(new Set())
    setCatalogSubscribedMemberIds(new Set())
    setCatalogUnsubscribingTrackKeys(new Set())
    setIsSidebarOpen(false)
    setStatsHistory(new Map())
    setRemoteStats(new Map())
  }, [session])

  useEffect(() => {
    const memberIds = new Set(room.remoteMembers.keys())
    setRemoteCatalogTracks((prev) => new Map([...prev].filter(([memberId]) => memberIds.has(memberId))))
    setRemoteCatalogSelections((prev) => new Map([...prev].filter(([memberId]) => memberIds.has(memberId))))
    setCatalogLoadingMemberIds((prev) => new Set([...prev].filter((memberId) => memberIds.has(memberId))))
    setCatalogSubscribedMemberIds((prev) => new Set([...prev].filter((memberId) => memberIds.has(memberId))))
    setCatalogUnsubscribingTrackKeys(
      (prev) => new Set([...prev].filter((key) => memberIds.has(key.slice(0, key.lastIndexOf(':')))))
    )
  }, [room.remoteMembers])

  useEffect(() => {
    statsSnapshotRef.current = {
      localMemberId: room.localMember.id,
      localVideoBitrate,
      localAudioBitrate,
      localCameraVideoSendTiming,
      localScreenShareVideoSendTiming,
      remoteMembers: Array.from(room.remoteMembers.values())
    }
  }, [
    localAudioBitrate,
    localVideoBitrate,
    localCameraVideoSendTiming,
    localScreenShareVideoSendTiming,
    room.localMember.id,
    room.remoteMembers
  ])

  useEffect(() => {
    const timer = window.setInterval(() => {
      const snapshot = statsSnapshotRef.current
      const sampledAt = Date.now()
      const { subscriber } = session.getMediaController()
      const latestRemoteStats = new Map<string, RemotePlaybackStats>()
      setStatsHistory((prev) => {
        const next = new Map<string, SidebarStatsSample[]>()
        const localSample: SidebarStatsSample = {
          timestamp: sampledAt,
          videoBitrateKbps: snapshot.localVideoBitrate,
          audioBitrateKbps: snapshot.localAudioBitrate,
          localCameraSend: snapshot.localCameraVideoSendTiming,
          localScreenShareSend: snapshot.localScreenShareVideoSendTiming
        }
        next.set(snapshot.localMemberId, appendStatsSample(prev.get(snapshot.localMemberId), localSample))
        for (const remoteMember of snapshot.remoteMembers) {
          const stats = subscriber.stats(remoteMember.id)
          latestRemoteStats.set(remoteMember.id, stats)
          const primary = stats.camera ?? stats.screenshare
          const remoteSample: SidebarStatsSample = {
            timestamp: sampledAt,
            videoBitrateKbps: stats.camera?.receivedKbps,
            screenShareBitrateKbps: stats.screenshare?.receivedKbps,
            audioBitrateKbps: stats.camera?.receivedAudioKbps,
            bufferMs: primary?.bufferMs,
            targetBufferMs: primary?.targetBufferMs,
            outputLatencyMs: primary?.outputLatencyMs,
            arrivalSpreadMs: primary?.arrivalSpreadMs,
            viewerDelayMs: primary?.viewerDelayMs,
            syncOffsetMs: primary?.syncOffsetMs
          }
          next.set(remoteMember.id, appendStatsSample(prev.get(remoteMember.id), remoteSample))
        }
        return next
      })
      setRemoteStats(latestRemoteStats)
    }, SIDEBAR_STATS_SAMPLE_INTERVAL_MS)

    return () => window.clearInterval(timer)
  }, [session])

  useSessionEventHandlers({
    session,
    roomName,
    userName,
    setRoom,
    setChatMessages
  })

  const applyRemoteCatalogTracks = (memberId: string, tracks: MeetingCatalogTrack[]) => {
    setRemoteCatalogTracks((prev) => {
      const next = new Map(prev)
      next.set(memberId, tracks)
      return next
    })
    setRemoteCatalogSelections((prev) => {
      const next = new Map(prev)
      const current = next.get(memberId) ?? {}
      next.set(
        memberId,
        Object.fromEntries(
          CATALOG_SUBSCRIBE_ROLES.map((role) => [role, ensureSelectedTrackName(current[role], tracks, role)])
        )
      )
      return next
    })
  }

  const handleLoadCatalogTracks = async (memberId: string) => {
    const member = room.remoteMembers.get(memberId)
    if (!member) {
      return
    }
    const { trackNamespace } = member
    if (catalogLoadingMemberIds.has(memberId) || catalogSubscribedMemberIds.has(memberId)) {
      return
    }

    setCatalogLoadingMemberIds((prev) => {
      const next = new Set(prev)
      next.add(memberId)
      return next
    })

    try {
      const tracks = await session.subscribeCatalog(
        trackNamespace,
        (updatedTracks) => applyRemoteCatalogTracks(memberId, updatedTracks),
        () =>
          setCatalogSubscribedMemberIds((prev) => {
            const next = new Set(prev)
            next.delete(memberId)
            return next
          })
      )
      applyRemoteCatalogTracks(memberId, tracks)
      setCatalogSubscribedMemberIds((prev) => {
        const next = new Set(prev)
        next.add(memberId)
        return next
      })
    } finally {
      setCatalogLoadingMemberIds((prev) => {
        const next = new Set(prev)
        next.delete(memberId)
        return next
      })
    }
  }

  useEffect(() => {
    catalogSubscribeBackoff.retain(room.remoteMembers)
    for (const memberId of room.remoteMembers.keys()) {
      if (catalogLoadingMemberIds.has(memberId) || catalogSubscribedMemberIds.has(memberId)) {
        continue
      }
      catalogSubscribeBackoff.run(memberId, () => handleLoadCatalogTracks(memberId))
    }
  }, [
    catalogLoadingMemberIds,
    catalogSubscribeBackoff,
    catalogSubscribeRetryTick,
    catalogSubscribedMemberIds,
    room.remoteMembers
  ])

  const handleSelectCatalogTrack = (memberId: string, role: CatalogSubscribeRole, trackName: string) => {
    const trackKey = buildTrackActionKey(memberId, role)
    if (trackName === AUTO_TRACK_SELECTION) {
      setManualTrackKeys((prev) => {
        const next = new Set(prev)
        next.delete(trackKey)
        return next
      })
      return
    }
    if (isSwitchableRole(role)) {
      setManualTrackKeys((prev) => new Set(prev).add(trackKey))
    }
    selectAndSwitchTrack(memberId, role, trackName)
  }

  adaptiveChoiceRef.current = (memberId, role, trackName) => {
    if (!manualTrackKeys.has(buildTrackActionKey(memberId, role))) {
      selectAndSwitchTrack(memberId, role, trackName)
    }
  }

  const selectAndSwitchTrack = (memberId: string, role: CatalogSubscribeRole, trackName: string) => {
    setRemoteCatalogSelections((prev) => {
      const next = new Map(prev)
      const current = next.get(memberId) ?? {}
      next.set(memberId, { ...current, [role]: trackName })
      return next
    })
    const member = room.remoteMembers.get(memberId)
    if (member && role !== 'chat' && member.subscribedTracks[role].isSubscribed) {
      void switchCatalogTrack(member, role, trackName).catch((error) => {
        console.error(`Failed to switch ${role} track for ${memberId}:`, error)
      })
    }
  }

  /// The ladder is the role's tracks of the same codec as `trackName`, the
  /// ones a picture can switch between without a gap.
  const buildVideoAdaptation = (
    memberId: string,
    role: CatalogSubscribeRole,
    trackName: string
  ): VideoAdaptation | undefined => {
    if (!isSwitchableRole(role)) {
      return undefined
    }
    const roleTracks = catalogTracksForRole(remoteCatalogTracks.get(memberId) ?? [], role)
    const codec = roleTracks.find((track) => track.name === trackName)?.codec
    const ladder = roleTracks
      .filter((track) => track.codec === codec && track.bitrate !== undefined)
      .map((track) => ({ name: track.name, bitrate: track.bitrate ?? 0, height: track.height }))
      .sort((a, b) => a.bitrate - b.bitrate)
    return ladder.length > 1
      ? { ladder, onPreferred: (choice) => adaptiveChoiceRef.current(memberId, role, choice) }
      : undefined
  }

  /// Returns undefined, after telling the user, when the browser cannot decode the track.
  const resolveTrackMediaConfig = async (
    memberId: string,
    role: CatalogSubscribeRole,
    trackName: string
  ): Promise<{ config: TrackMediaConfig | undefined } | undefined> => {
    const selectedTrack =
      role === 'chat'
        ? undefined
        : remoteCatalogTracks
            .get(memberId)
            ?.find((track) => track.name === trackName && track.role === (role === 'audio' ? 'audio' : 'video'))
    if ((role === 'video' || role === 'screenshare') && selectedTrack) {
      const unsupportedReason = await checkVideoDecoderSupport(selectedTrack)
      if (unsupportedReason) {
        console.error(`[meeting][subscribe] blocked unsupported decoder config for ${memberId}/${trackName}`, {
          role,
          reason: unsupportedReason,
          track: selectedTrack
        })
        window.alert(`Subscribe blocked: ${unsupportedReason}`)
        return undefined
      }
    }
    return {
      config:
        role !== 'chat'
          ? {
              codec: selectedTrack?.codec,
              samplerate: role === 'audio' ? selectedTrack?.samplerate : undefined,
              channelConfig: role === 'audio' ? selectedTrack?.channelConfig : undefined,
              initData: selectedTrack?.initData
            }
          : undefined
    }
  }

  const subscriptionEnded = (memberId: string, role: CatalogSubscribeRole) => (endedId: bigint) =>
    setRoom((currentRoom) =>
      updateMemberSubscriptionStateByRole(currentRoom, memberId, role, (track) =>
        track.subscribeId === endedId
          ? { ...track, subscribeId: undefined, isSubscribing: false, isSubscribed: false }
          : track
      )
    )

  const switchCatalogTrack = async (member: RemoteMember, role: CatalogSubscribeRole, trackName: string) => {
    const memberId = member.id
    const trackState = member.subscribedTracks[role]
    const previousId = trackState.subscribeId
    if (typeof previousId !== 'bigint' || trackState.isSubscribing) {
      return
    }
    const resolved = await resolveTrackMediaConfig(memberId, role, trackName)
    if (!resolved) {
      return
    }
    setRoom((currentRoom) =>
      updateMemberSubscriptionStateByRole(currentRoom, memberId, role, (track) => ({ ...track, isSubscribing: true }))
    )
    try {
      const requestId = await session.switchTrack(
        previousId,
        member.trackNamespace,
        trackName,
        role,
        resolved.config,
        subscriptionEnded(memberId, role),
        buildVideoAdaptation(memberId, role, trackName)
      )
      setRoom((currentRoom) =>
        updateMemberSubscriptionStateByRole(currentRoom, memberId, role, (track) => ({
          ...track,
          subscribeId: requestId,
          isSubscribing: false,
          isSubscribed: true
        }))
      )
    } catch (error) {
      // The previous track still plays unless the switch had to unsubscribe it first.
      const stillSubscribed = session.isSubscribed(previousId)
      setRoom((currentRoom) =>
        updateMemberSubscriptionStateByRole(currentRoom, memberId, role, (track) => ({
          ...track,
          isSubscribing: false,
          isSubscribed: stillSubscribed,
          subscribeId: stillSubscribed ? previousId : undefined
        }))
      )
      throw error
    }
  }

  const subscribeCatalogTrack = async (
    member: RemoteMember,
    role: CatalogSubscribeRole,
    trackName: string | undefined
  ) => {
    if (!trackName) {
      return
    }
    const memberId = member.id
    const resolved = await resolveTrackMediaConfig(memberId, role, trackName)
    if (!resolved) {
      return
    }
    const trackMediaConfig = resolved.config
    const trackState = member.subscribedTracks[role]
    const trackKey = buildTrackActionKey(memberId, role)
    if (trackState.isSubscribed || trackState.isSubscribing || catalogUnsubscribingTrackKeys.has(trackKey)) {
      return
    }
    // Optimistically mark as subscribing; the request id is unknown until
    // subscribe() returns it (issued internally by moqtClient).
    setRoom((currentRoom) =>
      updateMemberSubscriptionStateByRole(currentRoom, memberId, role, () => ({
        ...trackState,
        isSubscribing: true,
        isSubscribed: false,
        subscribeId: undefined
      }))
    )

    try {
      const requestId = await session.subscribe(
        member.trackNamespace,
        trackName,
        role,
        trackMediaConfig,
        subscriptionEnded(memberId, role),
        buildVideoAdaptation(memberId, role, trackName)
      )
      setRoom((currentRoom) =>
        updateMemberSubscriptionStateByRole(currentRoom, memberId, role, (track) => ({
          ...track,
          subscribeId: requestId,
          isSubscribing: false,
          isSubscribed: true
        }))
      )
    } catch (error) {
      setRoom((currentRoom) =>
        updateMemberSubscriptionStateByRole(currentRoom, memberId, role, (track) => ({
          ...track,
          isSubscribing: false
        }))
      )
      throw error
    }
  }

  const handleUnsubscribeCatalogTrack = async (memberId: string, role: CatalogSubscribeRole) => {
    const member = room.remoteMembers.get(memberId)
    if (!member) {
      return
    }
    const trackState = member.subscribedTracks[role]
    const subscribeId = trackState.subscribeId
    const trackKey = buildTrackActionKey(memberId, role)
    if (
      typeof subscribeId !== 'bigint' ||
      !trackState.isSubscribed ||
      trackState.isSubscribing ||
      catalogUnsubscribingTrackKeys.has(trackKey)
    ) {
      return
    }
    setCatalogUnsubscribingTrackKeys((prev) => {
      const next = new Set(prev)
      next.add(trackKey)
      return next
    })
    try {
      await session.unsubscribe(subscribeId)
      setRoom((currentRoom) =>
        updateMemberSubscriptionStateByRole(currentRoom, memberId, role, (track) => ({
          ...track,
          isSubscribing: false,
          isSubscribed: false
        }))
      )
    } catch (error) {
      console.error(`Failed to unsubscribe ${role} track for ${memberId}:`, error)
    } finally {
      setCatalogUnsubscribingTrackKeys((prev) => {
        const next = new Set(prev)
        next.delete(trackKey)
        return next
      })
    }
  }

  const remoteMemberIds = useMemo(() => Array.from(room.remoteMembers.keys()), [room.remoteMembers])
  const remoteSharingIds = useMemo(
    () =>
      new Set(
        remoteMemberIds.filter(
          (memberId) => catalogTracksForRole(remoteCatalogTracks.get(memberId) ?? [], 'screenshare').length > 0
        )
      ),
    [remoteCatalogTracks, remoteMemberIds]
  )
  const stage = useMemo(
    () =>
      arrangeStage({
        view: stageView,
        filmstripOpen,
        remoteMemberIds,
        remoteSharingIds,
        localSharing: screenShareEnabled,
        activeSpeakerId
      }),
    [activeSpeakerId, filmstripOpen, remoteMemberIds, remoteSharingIds, screenShareEnabled, stageView]
  )

  /// Every member's audio stays subscribed; a video track only while its tile is on the stage.
  /// Unsubscribing a hidden tile, such as one behind a closed filmstrip, lets the publisher stop encoding it.
  useEffect(() => {
    const trackKeys = new Map<string, true>()
    for (const member of room.remoteMembers.values()) {
      if (!catalogSubscribedMemberIds.has(member.id)) {
        continue
      }
      const tracks = remoteCatalogTracks.get(member.id) ?? []
      for (const role of AUTO_SUBSCRIBED_ROLES) {
        const trackKey = buildTrackActionKey(member.id, role)
        trackKeys.set(trackKey, true)
        const state = member.subscribedTracks[role]
        if (state.isSubscribing || catalogUnsubscribingTrackKeys.has(trackKey)) {
          continue
        }
        const offered = catalogTracksForRole(tracks, role).length > 0
        const wanted =
          offered &&
          (role === 'audio' || stage.visibleKeys.has(tileKey(member.id, role === 'video' ? 'camera' : 'screenshare')))
        if (wanted && !state.isSubscribed) {
          const selected = remoteCatalogSelections.get(member.id)?.[role]
          let trackName = selected
          // Under Auto a video track starts at the rendition its tile fits, once the tile has been laid out.
          if (selected && isSwitchableRole(role) && !manualTrackKeys.has(trackKey)) {
            const heightPx = tileHeights.get(tileKey(member.id, role === 'video' ? 'camera' : 'screenshare'))
            if (heightPx === undefined) {
              continue
            }
            const ladder = buildVideoAdaptation(member.id, role, selected)?.ladder ?? []
            trackName = capLadderToHeight(ladder, heightPx).at(-1)?.name ?? selected
            if (trackName !== selected) {
              selectAndSwitchTrack(member.id, role, trackName)
            }
          }
          mediaSubscribeBackoff.run(trackKey, () => subscribeCatalogTrack(member, role, trackName))
        } else if (!wanted && state.isSubscribed) {
          void handleUnsubscribeCatalogTrack(member.id, role)
        }
      }
    }
    mediaSubscribeBackoff.retain(trackKeys)
  }, [
    catalogSubscribedMemberIds,
    catalogUnsubscribingTrackKeys,
    mediaSubscribeBackoff,
    manualTrackKeys,
    mediaSubscribeRetryTick,
    remoteCatalogSelections,
    remoteCatalogTracks,
    room.remoteMembers,
    stage.visibleKeys,
    tileHeights
  ])

  useEffect(() => {
    chatSubscribeBackoff.retain(room.remoteMembers)
    for (const member of room.remoteMembers.values()) {
      const chatState = member.subscribedTracks.chat
      const chatTrackKey = buildTrackActionKey(member.id, 'chat')
      if (chatState.isSubscribed || chatState.isSubscribing || catalogUnsubscribingTrackKeys.has(chatTrackKey)) {
        continue
      }
      const selected = remoteCatalogSelections.get(member.id)
      const catalogChatTrackName = remoteCatalogTracks.get(member.id)?.find((track) => track.role === 'chat')?.name
      const chatTrackName = selected?.chat ?? catalogChatTrackName ?? 'chat'
      chatSubscribeBackoff.run(member.id, () => subscribeCatalogTrack(member, 'chat', chatTrackName))
    }
  }, [
    catalogUnsubscribingTrackKeys,
    chatSubscribeBackoff,
    chatSubscribeRetryTick,
    remoteCatalogSelections,
    remoteCatalogTracks,
    room.remoteMembers
  ])

  return (
    <div className="flex h-screen flex-col bg-slate-950 text-white">
      <RoomHeader
        roomName={roomName}
        members={Array.from(room.remoteMembers.values()).map((member) => ({
          id: member.id,
          name: member.name,
          speaking: speakingMemberIds.has(member.id),
          states: memberStates(
            member,
            catalogLoadingMemberIds.has(member.id),
            catalogSubscribedMemberIds.has(member.id),
            catalogUnsubscribingTrackKeys
          )
        }))}
      />
      <div className="flex min-h-0 flex-1">
        <main className="min-w-0 flex-1">
          <MemberGrid
            localMember={room.localMember}
            remoteMembers={Array.from(room.remoteMembers.values())}
            localVideoStream={localVideoStream}
            localScreenShareStream={localScreenShareStream}
            remotePictures={remotePictures}
            remoteStats={remoteStats}
            playoutSettings={playoutSettings}
            onChangePlayoutSettings={setPlayoutSettings}
            onToggleCamera={toggleCamera}
            onToggleScreenShare={toggleScreenShare}
            onToggleMicrophone={toggleMicrophone}
            cameraEnabled={cameraEnabled}
            screenShareEnabled={screenShareEnabled}
            microphoneEnabled={microphoneEnabled}
            cameraBusy={cameraBusy}
            microphoneBusy={microphoneBusy}
            videoDevices={videoDevices}
            audioDevices={audioDevices}
            selectedVideoDeviceId={selectedVideoDeviceId}
            selectedAudioDeviceId={selectedAudioDeviceId}
            onSelectVideoDevice={selectVideoDevice}
            onSelectAudioDevice={selectAudioDevice}
            videoEncodingOptions={{
              codecOptions: videoCodecOptions,
              resolutionOptions: videoResolutionOptions,
              bitrateOptions: videoBitrateOptions
            }}
            videoHardwareAccelerationOptions={videoHardwareAccelerationOptions}
            selectedVideoEncoding={selectedVideoEncoding}
            onSelectVideoEncoding={selectVideoEncoding}
            selectedScreenShareEncoding={selectedScreenShareEncoding}
            onSelectScreenShareEncoding={selectScreenShareEncoding}
            audioEncodingOptions={{
              codecOptions: audioCodecOptions,
              bitrateOptions: audioBitrateOptions,
              channelOptions: audioChannelOptions
            }}
            captureSettings={captureSettings}
            onChangeCaptureSettings={updateCaptureSettings}
            onApplyCaptureSettings={applyCaptureSettings}
            catalogTracks={catalogTracks}
            subscribedCatalogTracks={subscribedCatalogTracks}
            onAddCatalogTrack={addCatalogTrack}
            onUpdateCatalogTrack={updateCatalogTrack}
            onRemoveCatalogTrack={removeCatalogTrack}
            remoteCatalogTracks={remoteCatalogTracks}
            remoteCatalogSelections={remoteCatalogSelections}
            manualTrackKeys={manualTrackKeys}
            catalogLoadingMemberIds={catalogLoadingMemberIds}
            catalogSubscribedMemberIds={catalogSubscribedMemberIds}
            catalogUnsubscribingTrackKeys={catalogUnsubscribingTrackKeys}
            onSelectCatalogTrack={handleSelectCatalogTrack}
            statsHistory={statsHistory}
            stage={stage}
            speakingMemberIds={speakingMemberIds}
            filmstripOpen={filmstripOpen}
            onToggleView={() => setStageView(stage.view === 'gallery' ? 'speaker' : 'gallery')}
            onToggleFilmstrip={() => setFilmstripOpen((open) => !open)}
            chatOpen={isSidebarOpen}
            onToggleChat={() => setIsSidebarOpen((open) => !open)}
            onLeave={onLeave}
            onTileHeight={handleTileHeight}
          />
        </main>
        {isSidebarOpen && (
          <ChatSidebar
            messages={chatMessages}
            chatMessage={chatMessage}
            onMessageChange={setChatMessage}
            onSend={handleSendChatMessage}
            onToggle={() => setIsSidebarOpen(false)}
          />
        )}
      </div>
    </div>
  )
}

function appendStatsSample(
  current: SidebarStatsSample[] | undefined,
  sample: SidebarStatsSample
): SidebarStatsSample[] {
  const base = current ?? []
  const next = [...base, sample]
  if (next.length <= SIDEBAR_STATS_HISTORY_LIMIT) {
    return next
  }
  return next.slice(next.length - SIDEBAR_STATS_HISTORY_LIMIT)
}

function ensureSelectedTrackName(
  selectedTrackName: string | undefined,
  tracks: MeetingCatalogTrack[],
  role: CatalogSubscribeRole
): string | undefined {
  const roleTracks = catalogTracksForRole(tracks, role)
  if (selectedTrackName && roleTracks.some((track) => track.name === selectedTrackName)) {
    return selectedTrackName
  }
  return pickHighestBitrateTrack(roleTracks)?.name
}

async function checkVideoDecoderSupport({ codec, width, height }: MeetingCatalogTrack): Promise<string | null> {
  if (!codec) {
    return 'Track codec is missing in catalog.'
  }
  const config: VideoDecoderConfig = {
    codec,
    codedWidth: width,
    codedHeight: height,
    ...(codec.startsWith('avc') ? { hardwareAcceleration: 'prefer-software' } : {})
  }
  return checkCodecSupport(`VideoDecoder config ${codec}`, () => VideoDecoder.isConfigSupported(config))
}
