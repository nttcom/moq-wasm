import { FormEvent, useEffect, useRef, useState } from 'react'
import { LocalSession } from '../session/localSession'
import { Room } from '../types/room'
import { ChatMessage } from '../types/chat'
import { RemoteMember } from '../types/member'
import { ChatSidebar } from './ChatSidebar'
import { MemberGrid } from './MemberGrid'
import { RoomHeader } from './RoomHeader'
import { useSessionEventHandlers } from '../hooks/useSessionEventHandlers'
import { useCallMedia } from '../hooks/useCallMedia'
import type { CallCatalogTrack, CatalogSubscribeRole } from '../types/catalog'
import type { RemoteMediaStreams } from '../types/media'
import type { SidebarStatsSample } from '../types/stats'
import { updateSubscriptionState } from '../utils/state/roomState'

interface CallRoomProps {
  session: LocalSession
  onLeave: () => void
}

type CatalogSelections = {
  video?: string
  screenshare?: string
  audio?: string
  chat?: string
}

type StatsSnapshot = {
  localMemberId: string
  localVideoBitrate: number | null
  localAudioBitrate: number | null
  remoteMembers: RemoteMember[]
  remoteMedia: Map<string, RemoteMediaStreams>
}

const SIDEBAR_STATS_SAMPLE_INTERVAL_MS = 1000
const SIDEBAR_STATS_HISTORY_LIMIT = 120

export function CallRoom({ session, onLeave }: CallRoomProps) {
  const roomName = session.roomName
  const userName = session.localMember.name
  const [chatMessage, setChatMessage] = useState('')
  const [chatMessages, setChatMessages] = useState<ChatMessage[]>([])
  const [isSidebarOpen, setIsSidebarOpen] = useState(true)
  const [statsHistory, setStatsHistory] = useState<Map<string, SidebarStatsSample[]>>(new Map())
  const [remoteCatalogTracks, setRemoteCatalogTracks] = useState<Map<string, CallCatalogTrack[]>>(new Map())
  const [remoteCatalogSelections, setRemoteCatalogSelections] = useState<Map<string, CatalogSelections>>(new Map())
  const [catalogLoadingMemberIds, setCatalogLoadingMemberIds] = useState<Set<string>>(new Set())
  const [catalogSubscribedMemberIds, setCatalogSubscribedMemberIds] = useState<Set<string>>(new Set())
  const [catalogUnsubscribingTrackKeys, setCatalogUnsubscribingTrackKeys] = useState<Set<string>>(new Set())
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
    remoteMedia,
    toggleCamera,
    toggleScreenShare,
    toggleMicrophone,
    videoJitterConfigs,
    setVideoJitterBufferConfig,
    audioJitterConfigs,
    setAudioJitterBufferConfig,
    videoCodecOptions,
    videoResolutionOptions,
    videoBitrateOptions,
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
    addCatalogTrack,
    updateCatalogTrack,
    removeCatalogTrack
  } = useCallMedia(session)

  const statsSnapshotRef = useRef<StatsSnapshot>({
    localMemberId: session.localMember.id,
    localVideoBitrate,
    localAudioBitrate,
    remoteMembers: [],
    remoteMedia
  })

  const [room, setRoom] = useState<Room>(() => ({
    name: roomName,
    localMember: {
      ...session.localMember,
      publishedTracks: { ...session.localMember.publishedTracks }
    },
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
      localMember: {
        ...session.localMember,
        publishedTracks: { ...session.localMember.publishedTracks }
      },
      remoteMembers: new Map()
    })
    setRemoteCatalogTracks(new Map())
    setRemoteCatalogSelections(new Map())
    setCatalogLoadingMemberIds(new Set())
    setCatalogSubscribedMemberIds(new Set())
    setCatalogUnsubscribingTrackKeys(new Set())
    setIsSidebarOpen(true)
    setStatsHistory(new Map())
  }, [session])

  useEffect(() => {
    const memberIds = new Set(room.remoteMembers.keys())
    setRemoteCatalogTracks((prev) => filterMapByKeys(prev, memberIds))
    setRemoteCatalogSelections((prev) => filterMapByKeys(prev, memberIds))
    setCatalogLoadingMemberIds((prev) => filterSetByKeys(prev, memberIds))
    setCatalogSubscribedMemberIds((prev) => filterSetByKeys(prev, memberIds))
    setCatalogUnsubscribingTrackKeys((prev) => filterSetByTrackKeys(prev, memberIds))
  }, [room.remoteMembers])

  useEffect(() => {
    statsSnapshotRef.current = {
      localMemberId: room.localMember.id,
      localVideoBitrate,
      localAudioBitrate,
      remoteMembers: Array.from(room.remoteMembers.values()),
      remoteMedia
    }
  }, [localAudioBitrate, localVideoBitrate, remoteMedia, room.localMember.id, room.remoteMembers])

  useEffect(() => {
    const timer = window.setInterval(() => {
      const snapshot = statsSnapshotRef.current
      const sampledAt = Date.now()
      setStatsHistory((prev) => {
        const next = new Map<string, SidebarStatsSample[]>()
        const localSample: SidebarStatsSample = {
          timestamp: sampledAt,
          videoBitrateKbps: snapshot.localVideoBitrate,
          audioBitrateKbps: snapshot.localAudioBitrate
        }
        next.set(snapshot.localMemberId, appendStatsSample(prev.get(snapshot.localMemberId), localSample))
        for (const remoteMember of snapshot.remoteMembers) {
          const media = snapshot.remoteMedia.get(remoteMember.id)
          const remoteSample: SidebarStatsSample = {
            timestamp: sampledAt,
            videoBitrateKbps: media?.videoBitrateKbps,
            screenShareBitrateKbps: media?.screenShareBitrateKbps,
            audioBitrateKbps: media?.audioBitrateKbps,
            videoKeyframeIntervalFrames: media?.videoKeyframeIntervalFrames,
            screenShareKeyframeIntervalFrames: media?.screenShareKeyframeIntervalFrames,
            videoReceiveLatencyMs: media?.videoLatencyReceiveMs,
            screenShareReceiveLatencyMs: media?.screenShareLatencyReceiveMs,
            audioReceiveLatencyMs: media?.audioLatencyReceiveMs,
            videoRenderLatencyMs: media?.videoLatencyRenderMs,
            screenShareRenderLatencyMs: media?.screenShareLatencyRenderMs,
            audioRenderLatencyMs: media?.audioLatencyRenderMs,
            audioPlaybackQueueMs: media?.audioPlaybackQueueMs,
            videoRenderingRateFps: media?.videoRenderingRateFps,
            screenShareRenderingRateFps: media?.screenShareRenderingRateFps,
            audioRenderingRateFps: media?.audioRenderingRateFps
          }
          next.set(remoteMember.id, appendStatsSample(prev.get(remoteMember.id), remoteSample))
        }
        return next
      })
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

  const markTrackSubscribing = (subscribeId: bigint | undefined) => {
    if (typeof subscribeId !== 'bigint') {
      return
    }
    setRoom((currentRoom) =>
      updateSubscriptionState(currentRoom, subscribeId, (track) => ({
        ...track,
        isSubscribing: true
      }))
    )
  }

  const getTrackNamespace = (member: RemoteMember): string[] | null => {
    return (
      member.announcedTracks.video.trackNamespace ??
      member.announcedTracks.screenshare.trackNamespace ??
      member.announcedTracks.audio.trackNamespace ??
      member.announcedTracks.chat.trackNamespace ??
      null
    )
  }

  const getCatalogSubscribeId = (member: RemoteMember): bigint | null => {
    const chatSubscribeId = member.subscribedTracks.chat.subscribeId
    if (typeof chatSubscribeId !== 'bigint') {
      return null
    }
    return chatSubscribeId + 1n
  }

  const applyRemoteCatalogTracks = (memberId: string, tracks: CallCatalogTrack[]) => {
    setRemoteCatalogTracks((prev) => {
      const next = new Map(prev)
      next.set(memberId, tracks)
      return next
    })
    setRemoteCatalogSelections((prev) => {
      const next = new Map(prev)
      const current = next.get(memberId) ?? {}
      const selectedVideo = ensureSelectedTrackName(current.video, tracks, 'video')
      const selectedScreenShare = ensureSelectedTrackName(current.screenshare, tracks, 'screenshare')
      const selectedAudio = ensureSelectedTrackName(current.audio, tracks, 'audio')
      const selectedChat = ensureSelectedTrackName(current.chat, tracks, 'chat')
      next.set(memberId, {
        video: selectedVideo,
        screenshare: selectedScreenShare,
        audio: selectedAudio,
        chat: selectedChat
      })
      return next
    })
  }

  const handleLoadCatalogTracks = async (memberId: string) => {
    const member = room.remoteMembers.get(memberId)
    if (!member) {
      return
    }
    const trackNamespace = getTrackNamespace(member)
    const catalogSubscribeId = getCatalogSubscribeId(member)
    if (!trackNamespace || typeof catalogSubscribeId !== 'bigint') {
      return
    }
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
        catalogSubscribeId,
        catalogSubscribeId,
        trackNamespace,
        undefined,
        5000,
        (updatedTracks) => applyRemoteCatalogTracks(memberId, updatedTracks)
      )
      applyRemoteCatalogTracks(memberId, tracks)
      setCatalogSubscribedMemberIds((prev) => {
        const next = new Set(prev)
        next.add(memberId)
        return next
      })
    } catch (error) {
      console.error(`Failed to load catalog tracks for ${memberId}:`, error)
    } finally {
      setCatalogLoadingMemberIds((prev) => {
        const next = new Set(prev)
        next.delete(memberId)
        return next
      })
    }
  }

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
    memberId: string,
    trackNamespace: string[],
    role: CatalogSubscribeRole,
    trackName: string | undefined
  ) => {
    if (!trackName) {
      return
    }
    const trackCodec =
      role === 'audio' || role === 'video' || role === 'screenshare'
        ? remoteCatalogTracks
            .get(memberId)
            ?.find((track) => {
              if (track.name !== trackName) {
                return false
              }
              if (role === 'audio') {
                return track.role === 'audio'
              }
              return track.role === 'video'
            })?.codec
        : undefined
    const trackState = getSubscriptionStateByRole(member, role)
    const subscribeId = trackState.subscribeId
    const trackKey = buildTrackActionKey(memberId, role)
    if (
      trackState.isSubscribed ||
      trackState.isSubscribing ||
      catalogUnsubscribingTrackKeys.has(trackKey) ||
      typeof subscribeId !== 'bigint'
    ) {
      return
    }
    markTrackSubscribing(subscribeId)

    try {
      await session.subscribe(subscribeId, subscribeId, trackNamespace, trackName, undefined, role, trackCodec)
    } catch (error) {
      console.error(`Failed to subscribe ${role} track for ${memberId}:`, error)
      setRoom((currentRoom) =>
        updateSubscriptionState(currentRoom, subscribeId, (track) => ({
          ...track,
          isSubscribing: false
        }))
      )
    }
  }

  const handleUnsubscribeCatalogTrack = async (memberId: string, role: CatalogSubscribeRole) => {
    const member = room.remoteMembers.get(memberId)
    if (!member) {
      return
    }
    const trackState = getSubscriptionStateByRole(member, role)
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
      await session.unsubscribe(subscribeId, role)
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
    const trackNamespace = getTrackNamespace(member)
    if (!trackNamespace) {
      return
    }
    const selected = remoteCatalogSelections.get(memberId)
    const trackName = getSelectedTrackName(selected, role)

    try {
      await subscribeCatalogTrack(member, memberId, trackNamespace, role, trackName)
    } catch (error) {
      console.error(`Failed to subscribe ${role} track for ${memberId}:`, error)
    }
  }

  const handleToggleCamera = async () => {
    const enabled = await toggleCamera()
    session.localMember.publishedTracks.video = enabled || screenShareEnabled
    setRoom((current) => ({
      ...current,
      localMember: {
        ...current.localMember,
        publishedTracks: {
          ...current.localMember.publishedTracks,
          video: enabled || screenShareEnabled
        }
      }
    }))
  }

  const handleToggleMicrophone = async () => {
    const enabled = await toggleMicrophone()
    session.localMember.publishedTracks.audio = enabled
    setRoom((current) => ({
      ...current,
      localMember: {
        ...current.localMember,
        publishedTracks: {
          ...current.localMember.publishedTracks,
          audio: enabled
        }
      }
    }))
  }

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
            remoteMedia={remoteMedia}
            videoJitterConfigs={videoJitterConfigs}
            onChangeVideoJitterConfig={setVideoJitterBufferConfig}
            audioJitterConfigs={audioJitterConfigs}
            onChangeAudioJitterConfig={setAudioJitterBufferConfig}
            onToggleCamera={handleToggleCamera}
            onToggleScreenShare={async () => {
              const enabled = await toggleScreenShare()
              session.localMember.publishedTracks.video = enabled || cameraEnabled
              setRoom((current) => ({
                ...current,
                localMember: {
                  ...current.localMember,
                  publishedTracks: {
                    ...current.localMember.publishedTracks,
                    video: enabled || cameraEnabled
                  }
                }
              }))
            }}
            onToggleMicrophone={handleToggleMicrophone}
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
            audioEncodingOptions={{
              codecOptions: audioCodecOptions,
              bitrateOptions: audioBitrateOptions,
              channelOptions: audioChannelOptions
            }}
            captureSettings={captureSettings}
            onChangeCaptureSettings={updateCaptureSettings}
            onApplyCaptureSettings={applyCaptureSettings}
            catalogTracks={catalogTracks}
            onAddCatalogTrack={addCatalogTrack}
            onUpdateCatalogTrack={updateCatalogTrack}
            onRemoveCatalogTrack={removeCatalogTrack}
            remoteCatalogTracks={remoteCatalogTracks}
            remoteCatalogSelections={remoteCatalogSelections}
            catalogLoadingMemberIds={catalogLoadingMemberIds}
            catalogSubscribedMemberIds={catalogSubscribedMemberIds}
            catalogUnsubscribingTrackKeys={catalogUnsubscribingTrackKeys}
            onLoadCatalogTracks={handleLoadCatalogTracks}
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

function getTracksForRole(tracks: CallCatalogTrack[], role: CatalogSubscribeRole): CallCatalogTrack[] {
  if (role === 'audio') {
    return tracks.filter((track) => track.role === 'audio')
  }
  if (role === 'chat') {
    return tracks.filter((track) => track.role === 'chat')
  }
  return tracks.filter((track) =>
    role === 'screenshare'
      ? track.role === 'video' && track.name.trim().toLowerCase().startsWith('screenshare')
      : track.role === 'video' && !track.name.trim().toLowerCase().startsWith('screenshare')
  )
}

function pickDefaultTrackName(tracks: CallCatalogTrack[], role: CatalogSubscribeRole): string | undefined {
  const roleTracks = getTracksForRole(tracks, role)
  if (!roleTracks.length) {
    return undefined
  }
  const withBitrate = roleTracks
    .filter((track) => typeof track.bitrate === 'number')
    .sort((a, b) => (b.bitrate ?? 0) - (a.bitrate ?? 0))
  if (withBitrate.length) {
    return withBitrate[0].name
  }
  return roleTracks[0].name
}

function ensureSelectedTrackName(
  selectedTrackName: string | undefined,
  tracks: CallCatalogTrack[],
  role: CatalogSubscribeRole
): string | undefined {
  if (selectedTrackName && getTracksForRole(tracks, role).some((track) => track.name === selectedTrackName)) {
    return selectedTrackName
  }
  return pickDefaultTrackName(tracks, role)
}

function getSelectedTrackName(selected: CatalogSelections | undefined, role: CatalogSubscribeRole): string | undefined {
  if (!selected) {
    return undefined
  }
  if (role === 'video') {
    return selected.video
  }
  if (role === 'screenshare') {
    return selected.screenshare
  }
  if (role === 'chat') {
    return selected.chat
  }
  return selected.audio
}

function filterMapByKeys<T>(source: Map<string, T>, keys: Set<string>): Map<string, T> {
  const next = new Map<string, T>()
  for (const [key, value] of source.entries()) {
    if (keys.has(key)) {
      next.set(key, value)
    }
  }
  return next
}

function filterSetByKeys(source: Set<string>, keys: Set<string>): Set<string> {
  const next = new Set<string>()
  for (const key of source.values()) {
    if (keys.has(key)) {
      next.add(key)
    }
  }
  return next
}

function filterSetByTrackKeys(source: Set<string>, memberIds: Set<string>): Set<string> {
  const next = new Set<string>()
  for (const key of source.values()) {
    const separatorIndex = key.lastIndexOf(':')
    const memberId = separatorIndex >= 0 ? key.slice(0, separatorIndex) : key
    if (memberIds.has(memberId)) {
      next.add(key)
    }
  }
  return next
}

function buildTrackActionKey(memberId: string, role: CatalogSubscribeRole): string {
  return `${memberId}:${role}`
}

function getSubscriptionStateByRole(member: RemoteMember, role: CatalogSubscribeRole) {
  if (role === 'video') {
    return member.subscribedTracks.video
  }
  if (role === 'screenshare') {
    return member.subscribedTracks.screenshare
  }
  if (role === 'chat') {
    return member.subscribedTracks.chat
  }
  return member.subscribedTracks.audio
}
