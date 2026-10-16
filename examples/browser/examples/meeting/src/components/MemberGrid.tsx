import { LocalMember, RemoteMember } from '../types/member'
import { ReactNode, useEffect, useState } from 'react'
import type { PlayoutSettings } from '../types/playout'
import { PlayoutControls } from './PlayoutControls'
import { ControlBar } from './ControlBar'
import { ParticipantTile } from './ParticipantTile'
import { LOCAL_MEMBER_ID, type StageArrangement, type StageTile, galleryTileWidth } from '../utils/stageLayout'
import { DeviceSelector } from './DeviceSelector'
import type { CaptureSettingsState } from '../types/captureConstraints'
import { GetUserMediaForm } from './GetUserMediaForm'
import {
  CATALOG_SUBSCRIBE_ROLES,
  DEFAULT_AUDIO_STREAM_UPDATE_SETTINGS,
  DEFAULT_VIDEO_KEYFRAME_INTERVAL,
  type CatalogSelections,
  type CatalogSubscribeRole,
  type EditableMeetingCatalogTrack,
  type MeetingCatalogTrack,
  AUTO_TRACK_SELECTION,
  isSwitchableRole
} from '../types/catalog'
import { catalogTracksForRole } from '../media/meetingCatalog'
import { buildTrackActionKey } from '../utils/state/roomState'
import { checkCodecSupport } from '../utils/codecSupport'
import type { LocalStatsSample, RemoteStatsSample } from '../types/stats'
import { MeetingStats, RemoteMemberStatsCharts, SendStatsCharts } from './StatsCharts'
import type { RemotePictures } from '../media/mediaSubscriber'
import { formatLiveStats } from '@player/locLive'
import type {
  VideoCodecOption,
  VideoHardwareAccelerationOption,
  VideoResolutionOption,
  VideoEncodingSettings
} from '../types/videoEncoding'
import type { SubscribedCatalogTrack } from '../media/mediaPublisher'
import { formatAudioChannelConfig, parseAudioChannelCount } from '../../../../utils/media/audioChannelConfig'

type VideoEncodingOptionSet = {
  codecOptions: VideoCodecOption[]
  resolutionOptions: VideoResolutionOption[]
  bitrateOptions: { id: string; label: string; bitrate: number }[]
}

type AudioEncodingOptionSet = {
  codecOptions: { id: string; label: string; codec: string }[]
  bitrateOptions: { id: string; label: string; bitrate: number }[]
  channelOptions: { id: string; label: string; channels: number }[]
}

type CatalogTabKey = 'video' | 'screenshare' | 'audio'

const TRACK_KIND_OPTIONS: { id: CatalogTabKey; label: string }[] = [
  { id: 'video', label: 'Video' },
  { id: 'screenshare', label: 'Screenshare' },
  { id: 'audio', label: 'Audio' }
]
const DEVICE_MODAL_FIELD_CLASS = 'flex items-center justify-between gap-3 text-sm text-blue-50'

interface MemberGridProps {
  localMember: LocalMember
  remoteMembers: RemoteMember[]
  localVideoStream?: MediaStream | null
  localScreenShareStream?: MediaStream | null
  remotePictures: Map<string, RemotePictures>
  playoutSettings: Map<string, PlayoutSettings>
  onChangePlayoutSettings: (userId: string, settings: PlayoutSettings) => void
  onToggleCamera: () => void
  onToggleScreenShare: () => void
  onToggleMicrophone: () => void
  cameraEnabled: boolean
  screenShareEnabled: boolean
  microphoneEnabled: boolean
  cameraBusy: boolean
  microphoneBusy: boolean
  videoDevices: MediaDeviceInfo[]
  audioDevices: MediaDeviceInfo[]
  selectedVideoDeviceId: string | null
  selectedAudioDeviceId: string | null
  onSelectVideoDevice: (deviceId: string) => void
  onSelectAudioDevice: (deviceId: string) => void
  videoEncodingOptions: VideoEncodingOptionSet
  videoHardwareAccelerationOptions: VideoHardwareAccelerationOption[]
  selectedVideoEncoding: VideoEncodingSettings
  onSelectVideoEncoding: (settings: Partial<VideoEncodingSettings>) => Promise<void>
  selectedScreenShareEncoding: VideoEncodingSettings
  onSelectScreenShareEncoding: (settings: Partial<VideoEncodingSettings>) => Promise<void>
  audioEncodingOptions: AudioEncodingOptionSet
  captureSettings: CaptureSettingsState
  onChangeCaptureSettings: (settings: Partial<CaptureSettingsState>) => void
  onApplyCaptureSettings: () => void
  catalogTracks: EditableMeetingCatalogTrack[]
  subscribedCatalogTracks: SubscribedCatalogTrack[]
  onAddCatalogTrack: (track: Omit<EditableMeetingCatalogTrack, 'id'>) => void
  onUpdateCatalogTrack: (id: string, patch: Partial<EditableMeetingCatalogTrack>) => void
  onRemoveCatalogTrack: (id: string) => void
  remoteCatalogTracks: Map<string, MeetingCatalogTrack[]>
  remoteCatalogSelections: Map<string, CatalogSelections>
  manualTrackKeys: Set<string>
  catalogLoadingMemberIds: Set<string>
  catalogSubscribedMemberIds: Set<string>
  catalogUnsubscribingTrackKeys: Set<string>
  onSelectCatalogTrack: (memberId: string, role: CatalogSubscribeRole, trackName: string) => void
  localStatsHistory: LocalStatsSample[]
  remoteStatsHistory: Map<string, RemoteStatsSample[]>
  stage: StageArrangement
  speakingMemberIds: Set<string>
  filmstripOpen: boolean
  onToggleView: () => void
  onToggleFilmstrip: () => void
  chatOpen: boolean
  unreadChatMessageCount: number
  onToggleChat: () => void
  onLeave: () => void
  onTileHeight: (tileKey: string, heightPx: number) => void
}

export function MemberGrid({
  localMember,
  remoteMembers,
  localVideoStream,
  localScreenShareStream,
  remotePictures,
  playoutSettings,
  onChangePlayoutSettings,
  onToggleCamera,
  onToggleScreenShare,
  onToggleMicrophone,
  cameraEnabled,
  screenShareEnabled,
  microphoneEnabled,
  cameraBusy,
  microphoneBusy,
  videoDevices,
  audioDevices,
  selectedVideoDeviceId,
  selectedAudioDeviceId,
  onSelectVideoDevice,
  onSelectAudioDevice,
  videoEncodingOptions,
  videoHardwareAccelerationOptions,
  selectedVideoEncoding,
  onSelectVideoEncoding,
  selectedScreenShareEncoding,
  onSelectScreenShareEncoding,
  audioEncodingOptions,
  captureSettings,
  onChangeCaptureSettings,
  onApplyCaptureSettings,
  catalogTracks,
  subscribedCatalogTracks,
  onAddCatalogTrack,
  onUpdateCatalogTrack,
  onRemoveCatalogTrack,
  remoteCatalogTracks,
  remoteCatalogSelections,
  manualTrackKeys,
  catalogLoadingMemberIds,
  catalogSubscribedMemberIds,
  catalogUnsubscribingTrackKeys,
  onSelectCatalogTrack,
  localStatsHistory,
  remoteStatsHistory,
  stage,
  speakingMemberIds,
  filmstripOpen,
  onToggleView,
  onToggleFilmstrip,
  chatOpen,
  unreadChatMessageCount,
  onToggleChat,
  onLeave,
  onTileHeight
}: MemberGridProps) {
  const [isDeviceModalOpen, setIsDeviceModalOpen] = useState(false)
  const [isCatalogModalOpen, setIsCatalogModalOpen] = useState(false)
  const [isStatsModalOpen, setIsStatsModalOpen] = useState(false)
  const [detailsTarget, setDetailsTarget] = useState<string | null>(null)
  const detailsMember = remoteMembers.find((member) => member.id === detailsTarget)
  const [galleryRef, gallerySize] = useElementSize()
  const galleryTile = galleryTileWidth(stage.others.length, gallerySize.width, gallerySize.height, GALLERY_GAP_PX)

  const renderTile = (tile: StageTile) => {
    if (tile.memberId === LOCAL_MEMBER_ID) {
      const share = tile.source === 'screenshare'
      return (
        <ParticipantTile
          name={localMember.name}
          testId={share ? 'member-share-local' : 'member-card-local'}
          stream={share ? localScreenShareStream : localVideoStream}
          label={share ? 'Screen' : 'You'}
          speaking={false}
          onOpenDetails={() => setDetailsTarget(LOCAL_MEMBER_ID)}
        />
      )
    }
    const member = remoteMembers.find((candidate) => candidate.id === tile.memberId)
    if (!member) {
      return null
    }
    const pictures = remotePictures.get(member.id)
    if (tile.source === 'screenshare') {
      return (
        <ParticipantTile
          name={member.name}
          testId={`member-share-${member.name}`}
          picture={pictures?.screenshare ?? null}
          label="Screen"
          speaking={false}
          onHeight={(heightPx) => onTileHeight(tile.key, heightPx)}
          onOpenDetails={() => setDetailsTarget(member.id)}
        />
      )
    }
    const stats = remoteStatsHistory.get(member.id)?.at(-1)?.camera
    return (
      <ParticipantTile
        name={member.name}
        testId={`member-card-${member.name}`}
        menuTestId={`member-menu-${member.name}`}
        picture={pictures?.camera ?? null}
        speaking={speakingMemberIds.has(member.id)}
        onHeight={(heightPx) => onTileHeight(tile.key, heightPx)}
        footer={
          <span data-testid={`member-stats-${member.name}`}>{stats ? formatLiveStats(stats) : 'Awaiting media'}</span>
        }
        onOpenDetails={() => setDetailsTarget(member.id)}
      />
    )
  }

  return (
    <div className="flex h-full min-h-0 flex-col">
      <div className="flex min-h-0 flex-1 gap-3 p-3">
        {stage.view === 'gallery' ? (
          <div
            ref={galleryRef}
            className="flex min-h-0 flex-1 flex-wrap content-center items-center justify-center overflow-hidden"
            style={{ gap: GALLERY_GAP_PX }}
          >
            {stage.others.map((tile) => (
              <div key={tile.key} className="aspect-video" style={{ width: galleryTile }}>
                {renderTile(tile)}
              </div>
            ))}
          </div>
        ) : (
          <>
            <div className="flex min-h-0 min-w-0 flex-1 items-center justify-center">
              {stage.focus && (
                <div key={stage.focus.key} className="h-full w-full">
                  {renderTile(stage.focus)}
                </div>
              )}
            </div>
            {filmstripOpen && stage.others.length > 0 && (
              <div className="flex w-56 shrink-0 flex-col gap-3 overflow-y-auto" data-testid="filmstrip">
                {stage.others.map((tile) => (
                  <div key={tile.key} className="aspect-video w-full shrink-0">
                    {renderTile(tile)}
                  </div>
                ))}
              </div>
            )}
          </>
        )}
      </div>
      <ControlBar
        cameraEnabled={cameraEnabled}
        microphoneEnabled={microphoneEnabled}
        screenShareEnabled={screenShareEnabled}
        cameraBusy={cameraBusy}
        microphoneBusy={microphoneBusy}
        onToggleCamera={onToggleCamera}
        onToggleMicrophone={onToggleMicrophone}
        onToggleScreenShare={onToggleScreenShare}
        onOpenDevices={() => setIsDeviceModalOpen(true)}
        onOpenCatalog={() => setIsCatalogModalOpen(true)}
        onOpenStats={() => setIsStatsModalOpen(true)}
        view={stage.view}
        onToggleView={onToggleView}
        filmstripOpen={stage.view === 'speaker' ? filmstripOpen : undefined}
        onToggleFilmstrip={onToggleFilmstrip}
        chatOpen={chatOpen}
        unreadChatMessageCount={unreadChatMessageCount}
        onToggleChat={onToggleChat}
        onLeave={onLeave}
      />
      {isDeviceModalOpen && (
        <DeviceModal title="Select Devices" onClose={() => setIsDeviceModalOpen(false)}>
          <div className="space-y-4">
            <DeviceSelector
              label="Camera"
              devices={videoDevices}
              selectedDeviceId={selectedVideoDeviceId}
              onChange={onSelectVideoDevice}
            />
            <DeviceSelector
              label="Microphone"
              devices={audioDevices}
              selectedDeviceId={selectedAudioDeviceId}
              onChange={onSelectAudioDevice}
            />
            <GetUserMediaForm
              settings={captureSettings}
              onChange={onChangeCaptureSettings}
              onApply={onApplyCaptureSettings}
              cameraBusy={cameraBusy}
              microphoneBusy={microphoneBusy}
              resolutionOptions={videoEncodingOptions.resolutionOptions}
            />
            <div className="space-y-3 rounded-md border border-white/10 bg-white/5 p-3">
              <div className="text-sm font-semibold text-white">Video encoder hardware acceleration</div>
              <OptionSelect
                label="Camera"
                className={DEVICE_MODAL_FIELD_CLASS}
                options={videoHardwareAccelerationOptions}
                value={selectedVideoEncoding.hardwareAcceleration}
                onChange={(option) => void onSelectVideoEncoding({ hardwareAcceleration: option.value })}
              />
              <OptionSelect
                label="Screen share"
                className={DEVICE_MODAL_FIELD_CLASS}
                options={videoHardwareAccelerationOptions}
                value={selectedScreenShareEncoding.hardwareAcceleration}
                onChange={(option) => void onSelectScreenShareEncoding({ hardwareAcceleration: option.value })}
              />
            </div>
          </div>
        </DeviceModal>
      )}
      {isCatalogModalOpen && (
        <DeviceModal title="Catalogs" onClose={() => setIsCatalogModalOpen(false)} size="wide">
          <CatalogTrackEditor
            tracks={catalogTracks}
            videoEncodingOptions={videoEncodingOptions}
            videoHardwareAccelerationOptions={videoHardwareAccelerationOptions}
            audioEncodingOptions={audioEncodingOptions}
            onAddTrack={onAddCatalogTrack}
            onUpdateTrack={onUpdateCatalogTrack}
            onRemoveTrack={onRemoveCatalogTrack}
          />
        </DeviceModal>
      )}
      {isStatsModalOpen && (
        <DeviceModal title="Stats" onClose={() => setIsStatsModalOpen(false)} size="wide">
          <MeetingStats
            localSamples={localStatsHistory}
            remoteMembers={remoteMembers.map((member) => ({
              id: member.id,
              name: member.name,
              samples: remoteStatsHistory.get(member.id) ?? []
            }))}
          />
        </DeviceModal>
      )}
      {detailsTarget === LOCAL_MEMBER_ID && (
        <DeviceModal title={`${localMember.name} (You)`} onClose={() => setDetailsTarget(null)} size="wide">
          <div className="space-y-4">
            <CatalogsPanel subscribedTracks={subscribedCatalogTracks} />
            <SendStatsCharts samples={localStatsHistory} />
          </div>
        </DeviceModal>
      )}
      {detailsMember && (
        <DeviceModal title={detailsMember.name} onClose={() => setDetailsTarget(null)} size="wide">
          <div className="space-y-4">
            <div className="grid gap-4 lg:grid-cols-2">
              <RemoteCatalogSubscribePanel
                member={detailsMember}
                tracks={remoteCatalogTracks.get(detailsMember.id) ?? []}
                selected={remoteCatalogSelections.get(detailsMember.id)}
                manualTrackKeys={manualTrackKeys}
                isLoading={catalogLoadingMemberIds.has(detailsMember.id)}
                isCatalogSubscribed={catalogSubscribedMemberIds.has(detailsMember.id)}
                unsubscribingTrackKeys={catalogUnsubscribingTrackKeys}
                onSelectTrack={(role, trackName) => onSelectCatalogTrack(detailsMember.id, role, trackName)}
              />
              <div className="space-y-3 rounded-md border border-white/10 bg-white/5 p-3">
                <div className="text-sm font-semibold text-blue-100">Playout</div>
                <PlayoutControls
                  value={playoutSettings.get(detailsMember.id)}
                  onChange={(settings) => onChangePlayoutSettings(detailsMember.id, settings)}
                />
              </div>
            </div>
            <RemoteMemberStatsCharts samples={remoteStatsHistory.get(detailsMember.id) ?? []} />
          </div>
        </DeviceModal>
      )}
    </div>
  )
}

const GALLERY_GAP_PX = 12

function useElementSize(): [(element: HTMLDivElement | null) => void, { width: number; height: number }] {
  const [element, setElement] = useState<HTMLDivElement | null>(null)
  const [size, setSize] = useState({ width: 0, height: 0 })
  useEffect(() => {
    if (!element) {
      return
    }
    const observer = new ResizeObserver(([entry]) => {
      setSize({ width: entry.contentRect.width, height: entry.contentRect.height })
    })
    observer.observe(element)
    return () => observer.disconnect()
  }, [element])
  return [setElement, size]
}

/// The catalog and per-role subscription state of a remote member as `data-*`
/// attributes, which the E2E tests read.
export function memberStates(
  member: RemoteMember,
  isCatalogLoading: boolean,
  isCatalogSubscribed: boolean,
  unsubscribingTrackKeys: Set<string>
): Record<string, string> {
  const states: Record<string, string> = {
    'data-catalog-status': isCatalogLoading ? 'Loading...' : isCatalogSubscribed ? 'Subscribed' : 'Waiting...'
  }
  for (const role of CATALOG_SUBSCRIBE_ROLES) {
    const { isSubscribed, isSubscribing } = member.subscribedTracks[role]
    states[`data-${role}-status`] = formatSubscribeStatus(
      isSubscribed,
      isSubscribing,
      unsubscribingTrackKeys.has(buildTrackActionKey(member.id, role))
    )
  }
  return states
}

function DeviceModal({
  title,
  onClose,
  children,
  size = 'compact'
}: {
  title: string
  onClose: () => void
  children: ReactNode
  size?: 'compact' | 'wide'
}) {
  const maxWidthClass = size === 'wide' ? 'max-w-6xl' : 'max-w-md'
  return (
    <div className="fixed inset-0 z-50 flex items-center justify-center bg-black/60 px-4 py-6">
      <div
        className={`w-full ${maxWidthClass} max-h-[90vh] overflow-y-auto rounded-2xl bg-slate-900 p-6 text-white shadow-2xl`}
      >
        <div className="mb-4 flex items-center justify-between">
          <h4 className="text-lg font-semibold">{title}</h4>
          <button
            onClick={onClose}
            data-testid="modal-close-button"
            className="rounded-md bg-white/10 px-3 py-1 text-sm text-blue-100 transition hover:bg-white/20"
          >
            Close
          </button>
        </div>
        {children}
      </div>
    </div>
  )
}

function CatalogTrackEditor({
  tracks,
  videoEncodingOptions,
  videoHardwareAccelerationOptions,
  audioEncodingOptions,
  onAddTrack,
  onUpdateTrack,
  onRemoveTrack
}: {
  tracks: EditableMeetingCatalogTrack[]
  videoEncodingOptions: VideoEncodingOptionSet
  videoHardwareAccelerationOptions: VideoHardwareAccelerationOption[]
  audioEncodingOptions: AudioEncodingOptionSet
  onAddTrack: (track: Omit<EditableMeetingCatalogTrack, 'id'>) => void
  onUpdateTrack: (id: string, patch: Partial<EditableMeetingCatalogTrack>) => void
  onRemoveTrack: (id: string) => void
}) {
  const [activeTab, setActiveTab] = useState<CatalogTabKey>('video')
  const [isAddingTrack, setIsAddingTrack] = useState(false)
  const [addTrackKind, setAddTrackKind] = useState<CatalogTabKey>('video')
  const [draftConfigError, setDraftConfigError] = useState<string | null>(null)
  const [isCheckingDraftConfig, setIsCheckingDraftConfig] = useState(false)
  const [videoDraft, setVideoDraft] = useState(() => createVideoTrackDraft(videoEncodingOptions))
  const [screenshareDraft, setScreenshareDraft] = useState(() => createScreenShareTrackDraft(videoEncodingOptions))
  const [audioDraft, setAudioDraft] = useState(() => createAudioTrackDraft(audioEncodingOptions))

  const tabItems: Array<{
    key: CatalogTabKey
    title: string
    description: string
    tracks: EditableMeetingCatalogTrack[]
  }> = [
    {
      key: 'video',
      title: 'Video',
      description: 'Camera tracks',
      tracks: catalogTracksForRole(tracks, 'video')
    },
    {
      key: 'screenshare',
      title: 'Screenshare',
      description: 'Display capture tracks',
      tracks: catalogTracksForRole(tracks, 'screenshare')
    },
    {
      key: 'audio',
      title: 'Audio',
      description: 'Microphone tracks',
      tracks: catalogTracksForRole(tracks, 'audio')
    }
  ]
  const currentTab = tabItems.find((tab) => tab.key === activeTab) ?? tabItems[0]
  const currentDraft =
    addTrackKind === 'video' ? videoDraft : addTrackKind === 'screenshare' ? screenshareDraft : audioDraft
  const currentVideoResolutionOptions =
    currentDraft.role === 'video'
      ? getSupportedVideoResolutionOptions(videoEncodingOptions, currentDraft.codec)
      : videoEncodingOptions.resolutionOptions

  const setCurrentDraft = (patch: TrackPatch) => {
    setDraftConfigError(null)
    if (addTrackKind === 'video') {
      setVideoDraft((prev) => ({ ...prev, ...patch }))
      return
    }
    if (addTrackKind === 'screenshare') {
      setScreenshareDraft((prev) => ({ ...prev, ...patch }))
      return
    }
    setAudioDraft((prev) => ({ ...prev, ...patch }))
  }

  const addDraftTrack = async () => {
    const normalizedName = (currentDraft.name || '').trim()
    if (!normalizedName) {
      return
    }
    if (currentDraft.role === 'video') {
      setIsCheckingDraftConfig(true)
      try {
        const unsupportedReason = await checkVideoEncoderSupport(currentDraft)
        if (unsupportedReason) {
          setDraftConfigError(unsupportedReason)
          return
        }
      } finally {
        setIsCheckingDraftConfig(false)
      }
    }
    onAddTrack({
      ...currentDraft,
      name: normalizedName,
      label: (currentDraft.label || normalizedName).trim() || normalizedName
    })
    if (addTrackKind === 'video') {
      setVideoDraft(createVideoTrackDraft(videoEncodingOptions))
    } else if (addTrackKind === 'screenshare') {
      setScreenshareDraft(createScreenShareTrackDraft(videoEncodingOptions))
    } else {
      setAudioDraft(createAudioTrackDraft(audioEncodingOptions))
    }
    setActiveTab(addTrackKind)
    setIsAddingTrack(false)
  }

  const openTrackSetup = () => {
    setDraftConfigError(null)
    setAddTrackKind(activeTab)
    setIsAddingTrack(true)
  }

  return (
    <div className="space-y-4">
      <div className="flex items-center justify-between gap-3">
        <p className="text-sm text-blue-100">Video / Screenshare / Audio のTrackをここで管理します。</p>
        <button
          type="button"
          onClick={openTrackSetup}
          className="rounded bg-blue-600 px-3 py-1.5 text-xs font-semibold text-white hover:bg-blue-700"
        >
          + Track
        </button>
      </div>
      <div className="grid grid-cols-1 gap-2 sm:grid-cols-3">
        {tabItems.map((tab) => (
          <button
            key={tab.key}
            type="button"
            onClick={() => setActiveTab(tab.key)}
            className={`rounded-lg border px-3 py-2 text-left text-sm transition ${
              activeTab === tab.key
                ? 'border-blue-300 bg-blue-500/20 text-white'
                : 'border-white/10 bg-white/5 text-blue-100 hover:bg-white/10'
            }`}
          >
            <span className="font-semibold">{tab.title}</span>
            <span className="ml-2 text-xs text-blue-200">({tab.tracks.length})</span>
          </button>
        ))}
      </div>
      {isAddingTrack ? (
        <section className="space-y-3 rounded-lg border border-white/10 bg-white/[0.03] p-3">
          <div className="text-sm font-semibold text-white">Track Setup</div>
          <div className="grid grid-cols-1 gap-2 md:grid-cols-2">
            <OptionSelect
              label="Track Type"
              options={TRACK_KIND_OPTIONS}
              value={addTrackKind}
              onChange={(option) => setAddTrackKind(option.id)}
            />
            <div />
            <label className="flex flex-col gap-1 text-xs text-blue-100">
              <span>Name</span>
              <input
                className="rounded border border-white/10 bg-white/10 px-2 py-1 text-sm text-white"
                value={currentDraft.name}
                onChange={(event) => setCurrentDraft({ name: event.target.value })}
              />
            </label>
            <label className="flex flex-col gap-1 text-xs text-blue-100">
              <span>Label</span>
              <input
                className="rounded border border-white/10 bg-white/10 px-2 py-1 text-sm text-white"
                value={currentDraft.label}
                onChange={(event) => setCurrentDraft({ label: event.target.value })}
              />
            </label>
            {currentDraft.role === 'video' ? (
              <>
                <OptionSelect
                  label="Codec Preset"
                  options={videoEncodingOptions.codecOptions}
                  value={findOptionId(videoEncodingOptions.codecOptions, 'codec', currentDraft.codec)}
                  onChange={(option) => {
                    const nextResolution = pickSupportedVideoResolutionOption(
                      videoEncodingOptions,
                      option.codec,
                      currentDraft.width,
                      currentDraft.height,
                      '1080p'
                    )
                    setCurrentDraft({
                      codec: option.codec,
                      width: nextResolution?.width ?? currentDraft.width,
                      height: nextResolution?.height ?? currentDraft.height
                    })
                  }}
                />
                <OptionSelect
                  label="Resolution Preset"
                  options={currentVideoResolutionOptions}
                  value={findVideoResolutionOptionIdFromList(
                    currentVideoResolutionOptions,
                    currentDraft.width,
                    currentDraft.height
                  )}
                  onChange={(option) => setCurrentDraft({ width: option.width, height: option.height })}
                />
                <OptionSelect
                  label="Bitrate Preset"
                  className="col-span-full flex flex-col gap-1 text-xs text-blue-100"
                  options={videoEncodingOptions.bitrateOptions}
                  value={findOptionId(videoEncodingOptions.bitrateOptions, 'bitrate', currentDraft.bitrate)}
                  onChange={(option) => setCurrentDraft({ bitrate: option.bitrate })}
                />
                <label className="flex max-w-xs flex-col gap-1 text-xs text-blue-100">
                  <span>Framerate (fps)</span>
                  <input
                    type="number"
                    min={1}
                    max={120}
                    step={1}
                    value={normalizeTrackFramerate(currentDraft)}
                    onChange={(event) =>
                      setCurrentDraft({
                        framerate: toPositiveInteger(event.target.value, 30)
                      })
                    }
                    className="rounded border border-white/10 bg-white/10 px-2 py-1 text-sm text-white"
                  />
                </label>
                <OptionSelect
                  label="Hardware Acceleration"
                  options={videoHardwareAccelerationOptions}
                  value={currentDraft.hardwareAcceleration ?? 'prefer-software'}
                  onChange={(option) => setCurrentDraft({ hardwareAcceleration: option.value })}
                />
                <KeyframeIntervalField
                  className="col-span-full flex max-w-xs flex-col gap-1 text-xs text-blue-100"
                  track={currentDraft}
                  onChange={setCurrentDraft}
                />
              </>
            ) : (
              <>
                <OptionSelect
                  label="Codec Preset"
                  options={audioEncodingOptions.codecOptions}
                  value={findOptionId(audioEncodingOptions.codecOptions, 'codec', currentDraft.codec)}
                  onChange={(option) => setCurrentDraft({ codec: option.codec })}
                />
                <OptionSelect
                  label="Bitrate Preset"
                  options={audioEncodingOptions.bitrateOptions}
                  value={findOptionId(audioEncodingOptions.bitrateOptions, 'bitrate', currentDraft.bitrate)}
                  onChange={(option) => setCurrentDraft({ bitrate: option.bitrate })}
                />
                <OptionSelect
                  label="Channel Preset"
                  className="col-span-full flex flex-col gap-1 text-xs text-blue-100"
                  options={audioEncodingOptions.channelOptions}
                  value={findOptionId(
                    audioEncodingOptions.channelOptions,
                    'channels',
                    parseAudioChannelCount(currentDraft.channelConfig)
                  )}
                  onChange={(option) =>
                    setCurrentDraft({
                      channelConfig: formatAudioChannelConfig(option.channels),
                      samplerate: currentDraft.samplerate ?? 48_000
                    })
                  }
                />
                <AudioGroupUpdateFields
                  className="col-span-full space-y-2 rounded border border-white/10 bg-white/5 p-2"
                  radioName="draft-audio-stream-update-mode"
                  track={currentDraft}
                  onChange={setCurrentDraft}
                />
              </>
            )}
          </div>

          {draftConfigError ? (
            <div className="rounded border border-rose-400/30 bg-rose-500/10 px-3 py-2 text-xs text-rose-200">
              {draftConfigError}
            </div>
          ) : null}
          <div className="flex justify-start gap-2">
            <button
              type="button"
              className="rounded bg-blue-600 px-3 py-1.5 text-xs font-semibold text-white hover:bg-blue-700"
              onClick={() => void addDraftTrack()}
              disabled={isCheckingDraftConfig}
            >
              {isCheckingDraftConfig ? 'Checking...' : 'Add to Catalog'}
            </button>
            <button
              type="button"
              className="rounded bg-white/10 px-3 py-1.5 text-xs font-semibold text-blue-100 hover:bg-white/20"
              onClick={() => setIsAddingTrack(false)}
            >
              Cancel
            </button>
          </div>
        </section>
      ) : null}

      <section className="space-y-2 rounded-lg border border-white/10 bg-white/[0.02] p-3">
        <div className="text-sm font-semibold text-white">Catalog Tracks</div>
        {currentTab.tracks.length === 0 ? (
          <div className="text-xs text-blue-300">No tracks in this group.</div>
        ) : (
          <div className="space-y-2">
            {currentTab.tracks.map((track) => {
              const metadataEntries = buildCatalogTrackMetadataEntries(track)
              return (
                <div key={track.id} className="flex items-start justify-between gap-3 rounded-md bg-white/[0.04] p-2">
                  <div className="min-w-0">
                    <div className="truncate text-sm font-medium text-white">{track.label || track.name}</div>
                    <div className="truncate text-xs text-blue-200">{formatCatalogTrackLabel(track)}</div>
                    {metadataEntries.length > 0 ? (
                      <dl className="mt-1 grid grid-cols-1 gap-x-3 gap-y-0.5 text-[11px] sm:grid-cols-2">
                        {metadataEntries.map((entry) => (
                          <div key={`${track.id}-${entry.key}`} className="flex min-w-0 items-baseline gap-1">
                            <dt className="shrink-0 text-blue-300">{entry.label}:</dt>
                            <dd className="truncate text-blue-100">{entry.value}</dd>
                          </div>
                        ))}
                      </dl>
                    ) : null}
                    <div className="mt-2">
                      {track.role === 'video' ? (
                        <KeyframeIntervalField track={track} onChange={(patch) => onUpdateTrack(track.id, patch)} />
                      ) : (
                        <AudioGroupUpdateFields
                          radioName={`audio-stream-update-mode-${track.id}`}
                          track={track}
                          onChange={(patch) => onUpdateTrack(track.id, patch)}
                        />
                      )}
                    </div>
                  </div>
                  <button
                    type="button"
                    className="rounded bg-red-500/80 px-2 py-1 text-xs font-semibold text-white hover:bg-red-500"
                    onClick={() => onRemoveTrack(track.id)}
                  >
                    Remove
                  </button>
                </div>
              )
            })}
          </div>
        )}
      </section>
    </div>
  )
}

function OptionSelect<T extends { id: string; label: string }>({
  label,
  options,
  value,
  onChange,
  className = 'flex flex-col gap-1 text-xs text-blue-100'
}: {
  label: string
  options: T[]
  value: string
  onChange: (option: T) => void
  className?: string
}) {
  return (
    <label className={className}>
      <span>{label}</span>
      <select
        className="rounded border border-white/10 bg-white/10 px-2 py-1 text-sm text-white"
        value={value}
        onChange={(event) => {
          const option = options.find((entry) => entry.id === event.target.value)
          if (option) {
            onChange(option)
          }
        }}
      >
        {options.map((option) => (
          <option key={option.id} value={option.id} className="bg-slate-900 text-white">
            {option.label}
          </option>
        ))}
      </select>
    </label>
  )
}

type TrackPatch = Partial<Omit<EditableMeetingCatalogTrack, 'id'>>

function KeyframeIntervalField({
  track,
  onChange,
  className = 'flex max-w-xs flex-col gap-1 text-xs text-blue-100'
}: {
  track: Pick<MeetingCatalogTrack, 'keyframeInterval'>
  onChange: (patch: TrackPatch) => void
  className?: string
}) {
  return (
    <label className={className}>
      <span>Keyframe Interval (frames)</span>
      <input
        type="number"
        min={1}
        step={1}
        value={normalizeTrackKeyframeInterval(track)}
        onChange={(event) =>
          onChange({ keyframeInterval: toPositiveInteger(event.target.value, DEFAULT_VIDEO_KEYFRAME_INTERVAL) })
        }
        className="rounded border border-white/10 bg-white/10 px-2 py-1 text-sm text-white"
      />
    </label>
  )
}

function AudioGroupUpdateFields({
  track,
  radioName,
  onChange,
  className = 'space-y-2'
}: {
  track: Pick<MeetingCatalogTrack, 'audioStreamUpdateMode' | 'audioStreamUpdateIntervalSeconds'>
  radioName: string
  onChange: (patch: TrackPatch) => void
  className?: string
}) {
  const mode = resolveAudioStreamUpdateMode(track)
  return (
    <div className={className}>
      <div className="text-xs font-semibold text-white">Audio Group Update</div>
      <div className="flex flex-wrap items-center gap-4">
        <label className="inline-flex items-center gap-2 text-xs text-blue-100">
          <input
            type="radio"
            name={radioName}
            checked={mode === 'single'}
            onChange={() => onChange({ audioStreamUpdateMode: 'single' })}
          />
          <span>単一 Stream</span>
        </label>
        <label className="inline-flex items-center gap-2 text-xs text-blue-100">
          <input
            type="radio"
            name={radioName}
            checked={mode === 'interval'}
            onChange={() => onChange({ audioStreamUpdateMode: 'interval' })}
          />
          <span>N 秒ごとに更新</span>
        </label>
      </div>
      {mode === 'interval' ? (
        <label className="flex max-w-xs flex-col gap-1 text-xs text-blue-100">
          <span>更新間隔 (秒)</span>
          <input
            type="number"
            min={1}
            step={1}
            value={resolveAudioStreamUpdateIntervalSeconds(track)}
            onChange={(event) =>
              onChange({
                audioStreamUpdateIntervalSeconds: toPositiveInteger(
                  event.target.value,
                  DEFAULT_AUDIO_STREAM_UPDATE_SETTINGS.intervalSeconds
                )
              })
            }
            className="rounded border border-white/10 bg-white/10 px-2 py-1 text-sm text-white"
          />
        </label>
      ) : null}
    </div>
  )
}

function findOptionId<Option extends { id: string }, Key extends keyof Option>(
  options: Option[],
  key: Key,
  value: Option[Key] | undefined
): string {
  return options.find((entry) => entry[key] === value)?.id ?? options[0]?.id ?? ''
}

function createVideoTrackDraft(options: VideoEncodingOptionSet): Omit<EditableMeetingCatalogTrack, 'id'> {
  const codec = options.codecOptions[0]?.codec ?? 'avc1.42E01E'
  const resolution = pickSupportedVideoResolutionOption(options, codec, undefined, undefined, '1080p')
  const bitrate = options.bitrateOptions[0]?.bitrate ?? 800_000
  return {
    name: `camera_${Date.now()}`,
    label: 'Camera',
    role: 'video',
    codec,
    width: resolution?.width ?? 1280,
    height: resolution?.height ?? 720,
    bitrate,
    framerate: 30,
    hardwareAcceleration: 'prefer-software',
    keyframeInterval: DEFAULT_VIDEO_KEYFRAME_INTERVAL,
    samplerate: undefined,
    channelConfig: undefined,
    audioStreamUpdateMode: undefined,
    audioStreamUpdateIntervalSeconds: undefined,
    isLive: true
  }
}

function createScreenShareTrackDraft(options: VideoEncodingOptionSet): Omit<EditableMeetingCatalogTrack, 'id'> {
  const codec = options.codecOptions[0]?.codec ?? 'av01.0.08M.08'
  const resolution = pickSupportedVideoResolutionOption(options, codec, undefined, undefined, '1080p')
  const bitrate = options.bitrateOptions[0]?.bitrate ?? 1_000_000
  return {
    name: `screenshare_${Date.now()}`,
    label: 'Screenshare',
    role: 'video',
    codec,
    width: resolution?.width ?? 1920,
    height: resolution?.height ?? 1080,
    bitrate,
    framerate: 30,
    hardwareAcceleration: 'prefer-software',
    keyframeInterval: DEFAULT_VIDEO_KEYFRAME_INTERVAL,
    samplerate: undefined,
    channelConfig: undefined,
    audioStreamUpdateMode: undefined,
    audioStreamUpdateIntervalSeconds: undefined,
    isLive: true
  }
}

function createAudioTrackDraft(options: AudioEncodingOptionSet): Omit<EditableMeetingCatalogTrack, 'id'> {
  const codec = options.codecOptions[0]?.codec ?? 'opus'
  const bitrate = options.bitrateOptions[0]?.bitrate ?? 64_000
  const channels = options.channelOptions[0]?.channels ?? 1
  return {
    name: `audio_${Date.now()}`,
    label: 'Audio',
    role: 'audio',
    codec,
    bitrate,
    width: undefined,
    height: undefined,
    framerate: undefined,
    hardwareAcceleration: undefined,
    keyframeInterval: undefined,
    samplerate: 48_000,
    channelConfig: formatAudioChannelConfig(channels),
    audioStreamUpdateMode: DEFAULT_AUDIO_STREAM_UPDATE_SETTINGS.mode,
    audioStreamUpdateIntervalSeconds: DEFAULT_AUDIO_STREAM_UPDATE_SETTINGS.intervalSeconds,
    isLive: true
  }
}

function findVideoResolutionOptionIdFromList(
  options: Array<Pick<VideoResolutionOption, 'id' | 'width' | 'height'>>,
  width: number | undefined,
  height: number | undefined
): string {
  return options.find((entry) => entry.width === width && entry.height === height)?.id ?? options[0]?.id ?? ''
}

function getVideoCodecOptionByCodec(
  options: VideoEncodingOptionSet,
  codec: string | undefined
): VideoCodecOption | undefined {
  return options.codecOptions.find((entry) => entry.codec === codec)
}

function isResolutionSupportedByCodec(
  codecOption: VideoCodecOption | undefined,
  resolution: Pick<VideoResolutionOption, 'width' | 'height'>
): boolean {
  if (!codecOption?.maxEncodePixels) {
    return true
  }
  return resolution.width * resolution.height <= codecOption.maxEncodePixels
}

function getSupportedVideoResolutionOptions(
  options: VideoEncodingOptionSet,
  codec: string | undefined
): VideoResolutionOption[] {
  const codecOption = getVideoCodecOptionByCodec(options, codec)
  const filtered = options.resolutionOptions.filter((resolution) =>
    isResolutionSupportedByCodec(codecOption, resolution)
  )
  return filtered.length > 0 ? filtered : options.resolutionOptions
}

function pickSupportedVideoResolutionOption(
  options: VideoEncodingOptionSet,
  codec: string | undefined,
  width: number | undefined,
  height: number | undefined,
  preferredId?: string
): VideoResolutionOption | undefined {
  const supported = getSupportedVideoResolutionOptions(options, codec)
  const exact = supported.find((entry) => entry.width === width && entry.height === height)
  if (exact) {
    return exact
  }
  if (preferredId) {
    const preferred = supported.find((entry) => entry.id === preferredId)
    if (preferred) {
      return preferred
    }
  }
  return supported[0]
}

function normalizeTrackKeyframeInterval(track: Pick<MeetingCatalogTrack, 'keyframeInterval'>): number {
  return toPositiveInteger(track.keyframeInterval, DEFAULT_VIDEO_KEYFRAME_INTERVAL)
}

function normalizeTrackFramerate(track: Pick<MeetingCatalogTrack, 'framerate'>): number {
  return toPositiveInteger(track.framerate, 30)
}

function resolveAudioStreamUpdateMode(
  track: Pick<MeetingCatalogTrack, 'audioStreamUpdateMode'>
): 'single' | 'interval' {
  return track.audioStreamUpdateMode === 'single' ? 'single' : DEFAULT_AUDIO_STREAM_UPDATE_SETTINGS.mode
}

function resolveAudioStreamUpdateIntervalSeconds(
  track: Pick<MeetingCatalogTrack, 'audioStreamUpdateIntervalSeconds'>
): number {
  return toPositiveInteger(track.audioStreamUpdateIntervalSeconds, DEFAULT_AUDIO_STREAM_UPDATE_SETTINGS.intervalSeconds)
}

function toPositiveInteger(value: unknown, fallback: number): number {
  const parsed = typeof value === 'number' ? value : Number(value)
  if (!Number.isFinite(parsed) || parsed <= 0) {
    return fallback
  }
  return Math.floor(parsed)
}

const SUBSCRIBE_ROW_TITLES: Record<CatalogSubscribeRole, string> = {
  video: 'Video Track',
  screenshare: 'Screenshare Track',
  audio: 'Audio Track',
  chat: 'Chat Track'
}

function RemoteCatalogSubscribePanel({
  member,
  tracks,
  selected,
  isLoading,
  isCatalogSubscribed,
  unsubscribingTrackKeys,
  manualTrackKeys,
  onSelectTrack
}: {
  member: RemoteMember
  tracks: MeetingCatalogTrack[]
  selected?: CatalogSelections
  isLoading: boolean
  isCatalogSubscribed: boolean
  unsubscribingTrackKeys: Set<string>
  manualTrackKeys: Set<string>
  onSelectTrack: (role: CatalogSubscribeRole, trackName: string) => void
}) {
  const memberName = member.name
  const trackRows = CATALOG_SUBSCRIBE_ROLES.map((role) => {
    const roleTracks = catalogTracksForRole(tracks, role)
    const { isSubscribed, isSubscribing } = member.subscribedTracks[role]
    return {
      role,
      tracks: roleTracks,
      selectedTrack: selected?.[role] ?? roleTracks[0]?.name ?? '',
      subscribed: isSubscribed,
      subscribing: isSubscribing,
      unsubscribing: unsubscribingTrackKeys.has(buildTrackActionKey(member.id, role)),
      auto: isSwitchableRole(role) ? !manualTrackKeys.has(buildTrackActionKey(member.id, role)) : undefined
    }
  })
  const visibleTrackRows = trackRows.filter(
    (row) => row.tracks.length > 0 || row.subscribed || row.subscribing || row.unsubscribing
  )
  return (
    <div className="space-y-2 rounded-md border border-white/10 bg-white/5 p-3">
      <div className="flex items-center justify-between gap-2">
        <span className="text-sm font-semibold text-blue-100">Catalogs</span>
        <span data-testid={`catalog-status-${memberName}`} className="text-[11px] text-blue-300">
          {isLoading ? 'Loading...' : isCatalogSubscribed ? 'Subscribed' : 'Waiting...'}
        </span>
      </div>
      {visibleTrackRows.map((row) => {
        const busy = row.subscribing || row.unsubscribing
        return (
          <TrackSubscribeRow
            key={row.role}
            title={SUBSCRIBE_ROW_TITLES[row.role]}
            tracks={row.tracks}
            selected={row.selectedTrack}
            auto={row.auto}
            disabled={!isCatalogSubscribed || busy || row.role === 'chat'}
            statusText={formatSubscribeStatus(row.subscribed, row.subscribing, row.unsubscribing)}
            statusTestId={`track-status-${row.role}-${memberName}`}
            selectTestId={`track-select-${row.role}-${memberName}`}
            onChange={(trackName) => onSelectTrack(row.role, trackName)}
          />
        )
      })}
    </div>
  )
}

/// `auto` is undefined for a role without adaptation; otherwise the select
/// offers Auto and `auto` tells whether it is selected.
function TrackSubscribeRow({
  title,
  tracks,
  selected,
  auto,
  disabled,
  statusText,
  statusTestId,
  selectTestId,
  onChange
}: {
  title: string
  tracks: MeetingCatalogTrack[]
  selected: string
  auto?: boolean
  disabled: boolean
  statusText: string
  statusTestId?: string
  selectTestId?: string
  onChange: (trackName: string) => void
}) {
  return (
    <div className="space-y-1 rounded-md border border-white/10 bg-white/[0.03] p-2">
      <div className="flex items-center justify-between gap-2">
        <span className="text-xs font-semibold text-blue-100">{title}</span>
        <span data-testid={statusTestId} className="text-[11px] text-blue-300">
          {statusText}
        </span>
      </div>
      <div>
        <select
          value={auto ? AUTO_TRACK_SELECTION : selected}
          disabled={disabled}
          data-testid={selectTestId}
          onChange={(event) => onChange(event.target.value)}
          className="w-full min-w-0 rounded border border-white/10 bg-white/10 px-2 py-1 text-sm text-white outline-none focus:border-blue-300 focus:ring-1 focus:ring-blue-300 disabled:opacity-60"
        >
          {tracks.length === 0 ? (
            <option value="" className="bg-slate-900 text-white">
              No tracks
            </option>
          ) : (
            <>
              {auto !== undefined && (
                <option value={AUTO_TRACK_SELECTION} className="bg-slate-900 text-white">
                  {`Auto (${tracks.find((track) => track.name === selected)?.label ?? selected})`}
                </option>
              )}
              {tracks.map((track) => (
                <option key={track.name} value={track.name} className="bg-slate-900 text-white">
                  {formatCatalogTrackLabel(track)}
                </option>
              ))}
            </>
          )}
        </select>
      </div>
    </div>
  )
}

function formatSubscribeStatus(subscribed: boolean, subscribing: boolean, unsubscribing: boolean): string {
  if (unsubscribing) {
    return 'Unsubscribing...'
  }
  if (subscribing) {
    return 'Subscribing...'
  }
  return subscribed ? 'Subscribed' : 'Not subscribed'
}

function formatCatalogTrackLabel(track: MeetingCatalogTrack): string {
  const parts: string[] = [track.label || track.name]
  if (typeof track.bitrate === 'number') {
    parts.push(formatTrackBitrateForDisplay(track.bitrate))
  }
  if (track.role === 'video' && typeof track.width === 'number' && typeof track.height === 'number') {
    parts.push(`${track.width}x${track.height}`)
  }
  if (track.role === 'audio' && typeof track.samplerate === 'number') {
    parts.push(`${track.samplerate}Hz`)
  }
  return parts.join(' / ')
}

function buildCatalogTrackMetadataEntries(
  track: MeetingCatalogTrack
): Array<{ key: string; label: string; value: string }> {
  const entries: Array<{ key: string; label: string; value: string }> = []
  if (track.codec) {
    entries.push({ key: 'codec', label: 'Codec', value: track.codec })
  }
  if (typeof track.bitrate === 'number') {
    entries.push({ key: 'bitrate', label: 'Bitrate', value: formatTrackBitrateForDisplay(track.bitrate) })
  }
  if (track.role === 'video' && typeof track.width === 'number' && typeof track.height === 'number') {
    entries.push({ key: 'resolution', label: 'Resolution', value: `${track.width}x${track.height}` })
  }
  if (track.role === 'video' && typeof track.framerate === 'number') {
    entries.push({ key: 'framerate', label: 'Framerate', value: `${Math.round(track.framerate)}fps` })
  }
  if (track.role === 'video' && typeof track.codec === 'string' && track.codec.startsWith('avc')) {
    entries.push({ key: 'h264-format', label: 'H264 Format', value: 'annexb' })
  }
  if (track.role === 'video' && track.hardwareAcceleration) {
    entries.push({ key: 'hardware-accel', label: 'HW Accel', value: track.hardwareAcceleration })
  }
  if (track.role === 'video') {
    entries.push({
      key: 'keyframe-interval',
      label: 'Keyframe Interval',
      value: `${normalizeTrackKeyframeInterval(track)} frames`
    })
  }
  if (track.role === 'audio' && typeof track.samplerate === 'number') {
    entries.push({ key: 'samplerate', label: 'Sample Rate', value: `${track.samplerate}Hz` })
  }
  if (track.role === 'audio' && track.channelConfig) {
    entries.push({ key: 'channel', label: 'Channel', value: track.channelConfig })
  }
  if (track.role === 'audio') {
    const mode = resolveAudioStreamUpdateMode(track)
    entries.push({
      key: 'audio-update-mode',
      label: 'Audio Group Update',
      value: mode === 'single' ? 'Single stream' : `Every ${resolveAudioStreamUpdateIntervalSeconds(track)}s`
    })
  }
  if (typeof track.isLive === 'boolean') {
    entries.push({ key: 'live', label: 'Live', value: track.isLive ? 'true' : 'false' })
  }
  return entries
}

function formatTrackBitrateForDisplay(bitrate: number): string {
  if (bitrate >= 1_000_000) {
    const mbps = bitrate / 1_000_000
    const value = Number.isInteger(mbps) ? mbps.toFixed(0) : mbps.toFixed(1)
    return `${value}Mbps`
  }
  return `${Math.round(bitrate / 1000)}kbps`
}

async function checkVideoEncoderSupport(
  track: Pick<MeetingCatalogTrack, 'codec' | 'width' | 'height' | 'bitrate' | 'framerate' | 'hardwareAcceleration'>
): Promise<string | null> {
  const { codec, width, height, bitrate } = track
  if (!codec || !width || !height || !bitrate) {
    return 'Codec / resolution / bitrate must be set for video tracks.'
  }
  const config: VideoEncoderConfig = {
    codec,
    avc: codec.startsWith('avc') ? { format: 'annexb' } : undefined,
    width,
    height,
    bitrate,
    framerate: track.framerate ?? 30,
    hardwareAcceleration: track.hardwareAcceleration ?? 'prefer-software',
    scalabilityMode: 'L1T1',
    latencyMode: 'realtime'
  }
  return checkCodecSupport(`VideoEncoder config ${codec}`, () => VideoEncoder.isConfigSupported(config))
}

function CatalogsPanel({ subscribedTracks }: { subscribedTracks: SubscribedCatalogTrack[] }) {
  const activeTracks = [...subscribedTracks].sort((left, right) =>
    left.role !== right.role
      ? left.role.localeCompare(right.role)
      : (left.label || left.name).localeCompare(right.label || right.name)
  )
  return (
    <div className="space-y-3 rounded-md border border-white/10 bg-white/5 p-3">
      <div className="text-sm font-semibold text-blue-100">Catalogs</div>
      <div className="space-y-2 text-[10px] text-blue-100/90">
        <div className="text-[10px] font-medium text-blue-200/90">Subscribed</div>
        {activeTracks.length ? (
          <div className="grid grid-cols-1 gap-2 lg:grid-cols-2">
            {activeTracks.map((track) => (
              <TrackSettingTile
                key={track.name}
                title={`${track.label || track.name} (${formatTrackRoleLabel(track.role)})`}
                rows={[
                  ['Track', track.name],
                  ['Subscribers', `${track.subscriberCount}`],
                  ...buildCatalogTrackMetadataEntries(track).map<[string, string]>((entry) => [
                    entry.label,
                    entry.value
                  ])
                ]}
              />
            ))}
          </div>
        ) : (
          <div className="rounded border border-white/10 bg-white/[0.03] px-2 py-1 text-[10px] text-blue-200">
            No active subscribers
          </div>
        )}
      </div>
    </div>
  )
}

function TrackSettingTile({ title, rows }: { title: string; rows: [string, string][] }) {
  return (
    <div className="rounded border border-white/10 bg-white/[0.03] p-2 text-[10px] text-blue-100/90">
      <div className="mb-2 text-[10px] font-medium text-blue-200/90">{title}</div>
      <div className="grid grid-cols-1 gap-1 sm:grid-cols-2">
        {rows.map(([label, value]) => (
          <div
            key={`${title}-${label}`}
            className="flex items-center justify-between gap-2 rounded bg-white/[0.03] px-2 py-1"
          >
            <span className="text-blue-200/90">{label}</span>
            <span className="font-mono text-white/95">{value}</span>
          </div>
        ))}
      </div>
    </div>
  )
}

function formatTrackRoleLabel(role: MeetingCatalogTrack['role']): string {
  if (role === 'video') {
    return 'Video'
  }
  if (role === 'audio') {
    return 'Audio'
  }
  return 'Chat'
}
