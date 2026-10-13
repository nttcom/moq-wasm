import { FormEvent, useEffect, useRef, useState } from 'react'
import { LocalSession } from '../session/localSession'
import { Room } from '../types/room'
import { ChatMessage } from '../types/chat'
import { RemoteMember, type SubscriptionState } from '../types/member'
import { ChatSidebar } from './ChatSidebar'
import { MemberGrid } from './MemberGrid'
import { RoomHeader } from './RoomHeader'
import { useSessionEventHandlers } from '../hooks/useSessionEventHandlers'
import { useMeetingMedia } from '../hooks/useMeetingMedia'
import { useRetryBackoff } from '../hooks/useRetryBackoff'
import {
  CATALOG_SUBSCRIBE_ROLES,
  type MeetingCatalogTrack,
  type CatalogSubscribeRole,
  type TrackMediaConfig
} from '../types/catalog'
import { pickHighestBitrateTrack } from '../media/meetingCatalog'
import { isScreenShareTrackName } from '../utils/catalogTrackName'
import { checkCodecSupport } from '../utils/codecSupport'
import type { RemotePlaybackStats } from '../media/mediaSubscriber'
import type { SidebarStatsSample, VideoSendTiming } from '../types/stats'
import { buildTrackActionKey, updateSubscriptionState } from '../utils/state/roomState'

interface MeetingRoomProps {
  session: LocalSession
  onLeave: () => void
}

type CatalogSelections = Partial<Record<CatalogSubscribeRole, string>>

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

export function MeetingRoom({ session, onLeave }: MeetingRoomProps) {
  const roomName = session.roomName
  const userName = session.localMember.name
  const [chatMessage, setChatMessage] = useState('')
  const [chatMessages, setChatMessages] = useState<ChatMessage[]>([])
  const [isSidebarOpen, setIsSidebarOpen] = useState(true)
  const [statsHistory, setStatsHistory] = useState<Map<string, SidebarStatsSample[]>>(new Map())
  const [remoteStats, setRemoteStats] = useState<Map<string, RemotePlaybackStats>>(new Map())
  const [remoteCatalogTracks, setRemoteCatalogTracks] = useState<Map<string, MeetingCatalogTrack[]>>(new Map())
  const [remoteCatalogSelections, setRemoteCatalogSelections] = useState<Map<string, CatalogSelections>>(new Map())
  const [catalogLoadingMemberIds, setCatalogLoadingMemberIds] = useState<Set<string>>(new Set())
  const [catalogSubscribedMemberIds, setCatalogSubscribedMemberIds] = useState<Set<string>>(new Set())
  const [catalogUnsubscribingTrackKeys, setCatalogUnsubscribingTrackKeys] = useState<Set<string>>(new Set())
  const { backoff: chatSubscribeBackoff, retryTick: chatSubscribeRetryTick } = useRetryBackoff('chat-auto-subscribe')
  const { backoff: catalogSubscribeBackoff, retryTick: catalogSubscribeRetryTick } =
    useRetryBackoff('catalog-auto-subscribe')
  const {
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
    setIsSidebarOpen(true)
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
      const tracks = await session.subscribeCatalog(trackNamespace, (updatedTracks) =>
        applyRemoteCatalogTracks(memberId, updatedTracks)
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
    setRemoteCatalogSelections((prev) => {
      const next = new Map(prev)
      const current = next.get(memberId) ?? {}
      next.set(memberId, { ...current, [role]: trackName })
      return next
    })
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
        return
      }
    }
    const trackMediaConfig: TrackMediaConfig | undefined =
      role !== 'chat'
        ? {
            codec: selectedTrack?.codec,
            samplerate: role === 'audio' ? selectedTrack?.samplerate : undefined,
            channelConfig: role === 'audio' ? selectedTrack?.channelConfig : undefined,
            initData: selectedTrack?.initData
          }
        : undefined
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
      const requestId = await session.subscribe(member.trackNamespace, trackName, role, trackMediaConfig)
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
        updateSubscriptionState(currentRoom, subscribeId, (track) => ({
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

  const handleSubscribeCatalogTrack = async (memberId: string, role: CatalogSubscribeRole) => {
    const member = room.remoteMembers.get(memberId)
    if (!member) {
      return
    }
    try {
      await subscribeCatalogTrack(member, role, remoteCatalogSelections.get(memberId)?.[role])
    } catch (error) {
      console.error(`Failed to subscribe ${role} track for ${memberId}:`, error)
    }
  }

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
    <div className="min-h-screen bg-gradient-to-br from-gray-900 via-blue-900 to-gray-900 text-white">
      <div className="flex min-h-screen flex-col lg:flex-row">
        <main className="flex-1 px-6 py-10 lg:px-10 lg:py-12">
          <RoomHeader roomName={roomName} onLeave={onLeave} />
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
            catalogLoadingMemberIds={catalogLoadingMemberIds}
            catalogSubscribedMemberIds={catalogSubscribedMemberIds}
            catalogUnsubscribingTrackKeys={catalogUnsubscribingTrackKeys}
            onSelectCatalogTrack={handleSelectCatalogTrack}
            onSubscribeVideoTrack={(memberId) => handleSubscribeCatalogTrack(memberId, 'video')}
            onSubscribeScreenshareTrack={(memberId) => handleSubscribeCatalogTrack(memberId, 'screenshare')}
            onSubscribeAudioTrack={(memberId) => handleSubscribeCatalogTrack(memberId, 'audio')}
            onSubscribeChatTrack={(memberId) => handleSubscribeCatalogTrack(memberId, 'chat')}
            onUnsubscribeVideoTrack={(memberId) => handleUnsubscribeCatalogTrack(memberId, 'video')}
            onUnsubscribeScreenshareTrack={(memberId) => handleUnsubscribeCatalogTrack(memberId, 'screenshare')}
            onUnsubscribeAudioTrack={(memberId) => handleUnsubscribeCatalogTrack(memberId, 'audio')}
            onUnsubscribeChatTrack={(memberId) => handleUnsubscribeCatalogTrack(memberId, 'chat')}
            statsHistory={statsHistory}
          />
        </main>

        {isSidebarOpen ? (
          <ChatSidebar
            messages={chatMessages}
            chatMessage={chatMessage}
            onMessageChange={setChatMessage}
            onSend={handleSendChatMessage}
            onToggle={() => setIsSidebarOpen(false)}
          />
        ) : (
          <aside className="w-full border-t border-white/10 bg-gray-900/80 px-4 py-4 backdrop-blur lg:w-20 lg:border-l lg:border-t-0 lg:px-2 lg:py-6">
            <button
              type="button"
              onClick={() => setIsSidebarOpen(true)}
              className="mb-2 w-full rounded-lg bg-blue-600 px-2 py-2 text-xs font-semibold text-white transition hover:bg-blue-700"
              title="Open chat"
            >
              Chat
            </button>
          </aside>
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

function getTracksForRole(tracks: MeetingCatalogTrack[], role: CatalogSubscribeRole): MeetingCatalogTrack[] {
  if (role === 'audio' || role === 'chat') {
    return tracks.filter((track) => track.role === role)
  }
  return tracks.filter(
    (track) => track.role === 'video' && isScreenShareTrackName(track.name) === (role === 'screenshare')
  )
}

function updateMemberSubscriptionStateByRole(
  room: Room,
  memberId: string,
  role: CatalogSubscribeRole,
  updater: (track: SubscriptionState) => SubscriptionState
): Room {
  const member = room.remoteMembers.get(memberId)
  if (!member) {
    return room
  }
  const updatedMembers = new Map(room.remoteMembers)
  updatedMembers.set(memberId, {
    ...member,
    subscribedTracks: { ...member.subscribedTracks, [role]: updater(member.subscribedTracks[role]) }
  })
  return { ...room, remoteMembers: updatedMembers }
}

function ensureSelectedTrackName(
  selectedTrackName: string | undefined,
  tracks: MeetingCatalogTrack[],
  role: CatalogSubscribeRole
): string | undefined {
  const roleTracks = getTracksForRole(tracks, role)
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
