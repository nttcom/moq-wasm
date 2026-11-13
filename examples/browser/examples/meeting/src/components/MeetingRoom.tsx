import { FormEvent, useEffect, useMemo, useRef, useState } from 'react'
import { LocalSession } from '../session/localSession'
import { Room } from '../types/room'
import { ChatMessage } from '../types/chat'
import { RemoteMember } from '../types/member'
import { ChatSidebar } from './ChatSidebar'
import { MemberGrid, memberStates } from './MemberGrid'
import { useCaptions } from '../hooks/useCaptions'
import { useRoomRecording } from '../hooks/useRoomRecording'
import { resolveRecorderUrl } from '../recording/recorderApi'
import { RoomHeader } from './RoomHeader'
import { sampleRoom } from '../telemetry'
import { useSessionEventHandlers } from '../hooks/useSessionEventHandlers'
import { useMeetingMedia } from '../hooks/useMeetingMedia'
import { useRetryBackoff } from '../hooks/useRetryBackoff'
import { AUTO_TRACK_SELECTION, type CatalogSubscribeRole } from '../types/catalog'
import { REMOTE_VIDEO_SOURCES, type RemoteMemberView } from '../media/remotePlayers'
import type { RemoteStatsSample } from '../types/stats'
import { updateMemberChatState } from '../utils/state/roomState'
import { type StageView, arrangeStage, tileKey } from '../utils/stageLayout'

interface MeetingRoomProps {
  session: LocalSession
  onLeave: () => void
}

const SIDEBAR_STATS_SAMPLE_INTERVAL_MS = 1000
const SIDEBAR_STATS_HISTORY_LIMIT = 120

export function MeetingRoom({ session, onLeave }: MeetingRoomProps) {
  const roomName = session.roomName
  const userName = session.localMember.name
  const recording = useRoomRecording(
    resolveRecorderUrl(window.location.search, session.relayUrl),
    session.relayUrl,
    session.trackNamespacePrefix.join('/')
  )
  const [transcriberPresent, setTranscriberPresent] = useState(false)
  const captions = useCaptions(session, transcriberPresent)
  const [chatMessage, setChatMessage] = useState('')
  const [chatMessages, setChatMessages] = useState<ChatMessage[]>([])
  const [isSidebarOpen, setIsSidebarOpen] = useState(false)
  const [readChatMessageCount, setReadChatMessageCount] = useState(0)
  const unreadChatMessageCount = isSidebarOpen ? 0 : chatMessages.length - readChatMessageCount
  const [stageView, setStageView] = useState<StageView>('gallery')
  const [filmstripOpen, setFilmstripOpen] = useState(true)
  const [remoteStatsHistory, setRemoteStatsHistory] = useState<Map<string, RemoteStatsSample[]>>(new Map())
  const { backoff: chatSubscribeBackoff, retryTick: chatSubscribeRetryTick } = useRetryBackoff('chat-auto-subscribe')
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
    remoteViewVersion,
    toggleCamera,
    toggleScreenShare,
    toggleMicrophone,
    playoutSettings,
    setPlayoutSettings,
    videoResolutionOptions,
    videoHardwareAccelerationOptions,
    selectedVideoEncoding,
    selectVideoEncoding,
    selectedScreenShareEncoding,
    selectScreenShareEncoding,
    videoDevices,
    audioDevices,
    selectedVideoDeviceId,
    selectedAudioDeviceId,
    selectVideoDevice,
    selectAudioDevice,
    captureSettings,
    updateCaptureSettings,
    applyCaptureSettings,
    notice
  } = useMeetingMedia(session)

  const remoteMembersRef = useRef<RemoteMember[]>([])

  const [room, setRoom] = useState<Room>(() => ({
    name: roomName,
    localMember: session.localMember,
    remoteMembers: new Map()
  }))

  useEffect(() => {
    if (isSidebarOpen) {
      setReadChatMessageCount(chatMessages.length)
    }
  }, [isSidebarOpen, chatMessages.length])

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
    setIsSidebarOpen(false)
    setRemoteStatsHistory(new Map())
  }, [session])

  useEffect(() => {
    remoteMembersRef.current = Array.from(room.remoteMembers.values())
  }, [room.remoteMembers])

  useEffect(() => {
    const timer = window.setInterval(() => {
      const sampledAt = Date.now()
      const { players } = session
      setRemoteStatsHistory(
        (prev) =>
          new Map(
            remoteMembersRef.current.map((member) => [
              member.id,
              appendStatsSample(prev.get(member.id), { timestamp: sampledAt, ...players.stats(member.id) })
            ])
          )
      )
    }, SIDEBAR_STATS_SAMPLE_INTERVAL_MS)

    return () => window.clearInterval(timer)
  }, [session])

  useEffect(() => sampleRoom(session, () => remoteMembersRef.current), [session])

  useSessionEventHandlers({
    session,
    roomName,
    userName,
    setRoom,
    setChatMessages,
    setTranscriberPresent
  })

  const { players } = session
  const remoteMemberIds = useMemo(() => Array.from(room.remoteMembers.keys()), [room.remoteMembers])
  const remoteViews = useMemo(
    () =>
      new Map(
        remoteMemberIds.flatMap((memberId): [string, RemoteMemberView][] => {
          const view = players.view(memberId)
          return view ? [[memberId, view]] : []
        })
      ),
    // remoteViewVersion changes with the views `players` holds.
    [players, remoteMemberIds, remoteViewVersion]
  )
  const remoteSharingIds = useMemo(
    () =>
      new Set(
        remoteMemberIds.filter((memberId) => (remoteViews.get(memberId)?.screenshare.videoTracks.length ?? 0) > 0)
      ),
    [remoteMemberIds, remoteViews]
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

  /// Unsubscribing a hidden tile, such as one behind a closed filmstrip, lets the publisher stop encoding it.
  useEffect(() => {
    for (const memberId of remoteMemberIds) {
      for (const source of REMOTE_VIDEO_SOURCES) {
        players.setShown(memberId, source, stage.visibleKeys.has(tileKey(memberId, source)))
      }
    }
  }, [players, remoteMemberIds, stage.visibleKeys])

  const handleSelectTrack = (memberId: string, role: CatalogSubscribeRole, trackName: string) => {
    if (role === 'audio') {
      players.selectAudioTrack(memberId, trackName)
    } else if (role !== 'chat') {
      const source = role === 'video' ? 'camera' : 'screenshare'
      players.selectVideoTrack(memberId, source, trackName === AUTO_TRACK_SELECTION ? undefined : trackName)
    }
  }

  const subscribeChat = async (member: RemoteMember) => {
    if (member.chat.isSubscribed || member.chat.isSubscribing) {
      return
    }
    // Set before the first await: an effect of a render committed in that gap could otherwise
    // run after this subscribe finished, see neither flag and subscribe again.
    setRoom((currentRoom) =>
      updateMemberChatState(currentRoom, member.id, () => ({ isSubscribing: true, isSubscribed: false }))
    )
    try {
      const subscribeId = await session.subscribeChat(member.trackNamespace, (endedId) =>
        setRoom((currentRoom) =>
          updateMemberChatState(currentRoom, member.id, (chat) =>
            chat.subscribeId === endedId ? { isSubscribing: false, isSubscribed: false } : chat
          )
        )
      )
      setRoom((currentRoom) =>
        updateMemberChatState(currentRoom, member.id, () => ({ subscribeId, isSubscribing: false, isSubscribed: true }))
      )
    } catch (error) {
      setRoom((currentRoom) =>
        updateMemberChatState(currentRoom, member.id, (chat) => ({ ...chat, isSubscribing: false }))
      )
      throw error
    }
  }

  useEffect(() => {
    chatSubscribeBackoff.retain(room.remoteMembers)
    for (const member of room.remoteMembers.values()) {
      if (!member.chat.isSubscribed && !member.chat.isSubscribing) {
        chatSubscribeBackoff.run(member.id, () => subscribeChat(member))
      }
    }
  }, [chatSubscribeBackoff, chatSubscribeRetryTick, room.remoteMembers])

  return (
    <div className="flex h-screen flex-col bg-slate-950 text-white">
      <RoomHeader
        roomName={roomName}
        recording={recording?.active?.state}
        transcribing={transcriberPresent}
        transcriberStarting={captions?.status?.state === 'starting'}
        members={Array.from(room.remoteMembers.values()).map((member) => ({
          id: member.id,
          name: member.name,
          speaking: speakingMemberIds.has(member.id),
          states: memberStates(member, remoteViews.get(member.id))
        }))}
      />
      <div className="flex min-h-0 flex-1">
        <main className="min-w-0 flex-1">
          <MemberGrid
            localMember={room.localMember}
            remoteMembers={Array.from(room.remoteMembers.values())}
            localVideoStream={localVideoStream}
            localScreenShareStream={localScreenShareStream}
            remoteViews={remoteViews}
            remoteContainer={(memberId, source) => players.container(memberId, source)}
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
            videoResolutionOptions={videoResolutionOptions}
            videoHardwareAccelerationOptions={videoHardwareAccelerationOptions}
            selectedVideoEncoding={selectedVideoEncoding}
            onSelectVideoEncoding={selectVideoEncoding}
            selectedScreenShareEncoding={selectedScreenShareEncoding}
            onSelectScreenShareEncoding={selectScreenShareEncoding}
            captureSettings={captureSettings}
            onChangeCaptureSettings={updateCaptureSettings}
            onApplyCaptureSettings={applyCaptureSettings}
            onSelectTrack={handleSelectTrack}
            remoteStatsHistory={remoteStatsHistory}
            stage={stage}
            speakingMemberIds={speakingMemberIds}
            filmstripOpen={filmstripOpen}
            onToggleView={() => setStageView(stage.view === 'gallery' ? 'speaker' : 'gallery')}
            onToggleFilmstrip={() => setFilmstripOpen((open) => !open)}
            chatOpen={isSidebarOpen}
            unreadChatMessageCount={unreadChatMessageCount}
            onToggleChat={() => setIsSidebarOpen((open) => !open)}
            onLeave={onLeave}
            recording={recording}
            captions={captions}
          />
        </main>
        {isSidebarOpen && (
          <ChatSidebar
            messages={chatMessages}
            chatMessage={chatMessage}
            onMessageChange={setChatMessage}
            onSend={handleSendChatMessage}
            onToggle={() => setIsSidebarOpen(false)}
            captions={captions}
            transcribing={transcriberPresent}
            recording={recording}
          />
        )}
      </div>
      {notice && (
        <div
          role="status"
          className="pointer-events-none fixed bottom-20 left-1/2 z-50 -translate-x-1/2 rounded-lg border border-white/10 bg-slate-900 px-4 py-2 text-sm shadow-lg"
        >
          {notice}
        </div>
      )}
    </div>
  )
}

function appendStatsSample<S>(current: S[] | undefined, sample: S): S[] {
  const base = current ?? []
  const next = [...base, sample]
  if (next.length <= SIDEBAR_STATS_HISTORY_LIMIT) {
    return next
  }
  return next.slice(next.length - SIDEBAR_STATS_HISTORY_LIMIT)
}
