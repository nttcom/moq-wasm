import { LocalMember, RemoteMember } from '../types/member'
import { RemoteMediaStreams } from '../types/media'
import { MediaStreamVideo, MediaStreamAudio } from './MediaStreamElements'
import { ReactNode, useState } from 'react'
import type { VideoJitterConfig, AudioJitterConfig } from '../types/jitterBuffer'
import { VideoJitterBufferControls, AudioJitterBufferControls } from './JitterBufferControls'
import { Mic, MicOff, Monitor, Settings2, Video, VideoOff } from 'lucide-react'
import { DeviceSelector } from './DeviceSelector'
import type { CaptureSettingsState } from '../types/captureConstraints'
import { GetUserMediaForm } from './GetUserMediaForm'
import type { CallCatalogTrack, CatalogSubscribeRole, EditableCallCatalogTrack } from '../types/catalog'
import { isScreenShareTrackName } from '../utils/catalogTrackName'

type VideoEncodingOptionSet = {
  codecOptions: { id: string; label: string; codec: string }[]
  resolutionOptions: { id: string; label: string; width: number; height: number }[]
  bitrateOptions: { id: string; label: string; bitrate: number }[]
}

type AudioEncodingOptionSet = {
  codecOptions: { id: string; label: string; codec: string }[]
  bitrateOptions: { id: string; label: string; bitrate: number }[]
  channelOptions: { id: string; label: string; channels: number }[]
}

type CatalogTabKey = 'video' | 'screenshare' | 'audio'

interface MemberGridProps {
  localMember: LocalMember
  remoteMembers: RemoteMember[]
  localVideoStream?: MediaStream | null
  localScreenShareStream?: MediaStream | null
  localVideoBitrate?: number | null
  localAudioBitrate?: number | null
  remoteMedia: Map<string, RemoteMediaStreams>
  videoJitterConfigs: Map<string, VideoJitterConfig>
  onChangeVideoJitterConfig: (userId: string, config: Partial<VideoJitterConfig>) => void
  audioJitterConfigs: Map<string, AudioJitterConfig>
  onChangeAudioJitterConfig: (userId: string, config: Partial<AudioJitterConfig>) => void
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
  audioEncodingOptions: AudioEncodingOptionSet
  captureSettings: CaptureSettingsState
  onChangeCaptureSettings: (settings: Partial<CaptureSettingsState>) => void
  onApplyCaptureSettings: () => void
  catalogTracks: EditableCallCatalogTrack[]
  onAddCatalogTrack: (track: Omit<EditableCallCatalogTrack, 'id'>) => void
  onRemoveCatalogTrack: (id: string) => void
  remoteCatalogTracks: Map<string, CallCatalogTrack[]>
  remoteCatalogSelections: Map<string, { video?: string; screenshare?: string; audio?: string }>
  catalogLoadingMemberIds: Set<string>
  catalogSubscribedMemberIds: Set<string>
  catalogUnsubscribingTrackKeys: Set<string>
  onLoadCatalogTracks: (memberId: string) => void
  onSelectCatalogTrack: (memberId: string, role: CatalogSubscribeRole, trackName: string) => void
  onSubscribeVideoTrack: (memberId: string) => void
  onSubscribeScreenshareTrack: (memberId: string) => void
  onSubscribeAudioTrack: (memberId: string) => void
  onUnsubscribeVideoTrack: (memberId: string) => void
  onUnsubscribeScreenshareTrack: (memberId: string) => void
  onUnsubscribeAudioTrack: (memberId: string) => void
}

export function MemberGrid({
  localMember,
  remoteMembers,
  localVideoStream,
  localScreenShareStream,
  localVideoBitrate,
  localAudioBitrate,
  remoteMedia,
  videoJitterConfigs,
  onChangeVideoJitterConfig,
  audioJitterConfigs,
  onChangeAudioJitterConfig,
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
  audioEncodingOptions,
  captureSettings,
  onChangeCaptureSettings,
  onApplyCaptureSettings,
  catalogTracks,
  onAddCatalogTrack,
  onRemoveCatalogTrack,
  remoteCatalogTracks,
  remoteCatalogSelections,
  catalogLoadingMemberIds,
  catalogSubscribedMemberIds,
  catalogUnsubscribingTrackKeys,
  onLoadCatalogTracks,
  onSelectCatalogTrack,
  onSubscribeVideoTrack,
  onSubscribeScreenshareTrack,
  onSubscribeAudioTrack,
  onUnsubscribeVideoTrack,
  onUnsubscribeScreenshareTrack,
  onUnsubscribeAudioTrack
}: MemberGridProps) {
  const [isDeviceModalOpen, setIsDeviceModalOpen] = useState(false)
  const [isCatalogModalOpen, setIsCatalogModalOpen] = useState(false)
  const [jitterModalTarget, setJitterModalTarget] = useState<string | null>(null)

  const localOverlay = renderStatsOverlay('Encoded', { bitrate: localVideoBitrate }, { bitrate: localAudioBitrate })
  const localPrimaryVideoStream = localVideoStream ?? localScreenShareStream ?? null
  const localSecondaryVideoStream = localVideoStream && localScreenShareStream ? localScreenShareStream : null
  return (
    <section className="mt-8 grid gap-6 md:grid-cols-2 xl:grid-cols-2">
      <MemberCard
        key="local-member"
        title={localMember.name}
        headerActions={
          <div className="flex items-center gap-2">
            <ControlButton
              active={cameraEnabled}
              disabled={cameraBusy}
              onClick={onToggleCamera}
              activeLabel="Camera On"
              inactiveLabel="Camera Off"
              IconOn={Video}
              IconOff={VideoOff}
            />
            <ControlButton
              active={screenShareEnabled}
              disabled={cameraBusy}
              onClick={onToggleScreenShare}
              activeLabel="Sharing"
              inactiveLabel="Share"
              IconOn={Monitor}
              IconOff={Monitor}
            />
            <ControlButton
              active={microphoneEnabled}
              disabled={microphoneBusy}
              onClick={onToggleMicrophone}
              activeLabel="Mic On"
              inactiveLabel="Mic Off"
              IconOn={Mic}
              IconOff={MicOff}
            />
            <IconButton ariaLabel="Select devices" onClick={() => setIsDeviceModalOpen(true)} title="Device settings">
              <Settings2 className="h-4 w-4" />
            </IconButton>
            <button
              type="button"
              onClick={() => setIsCatalogModalOpen(true)}
              className="rounded-lg bg-white/10 px-2 py-2 text-xs font-semibold text-blue-100 transition hover:bg-white/20"
            >
              Catalog
            </button>
          </div>
        }
        videoStream={localPrimaryVideoStream}
        videoOverlay={localOverlay}
        secondaryVideoStream={localSecondaryVideoStream}
        secondaryVideoTitle={localSecondaryVideoStream ? 'Screen Share' : undefined}
        audioStream={null}
        muted
        placeholder="Camera disabled"
        secondaryPlaceholder="Screen share disabled"
        details={
          <>
            <TrackList
              title="Published Tracks"
              items={[
                { label: 'Chat', enabled: localMember.publishedTracks.chat },
                { label: 'Video', enabled: localMember.publishedTracks.video },
                { label: 'Audio', enabled: localMember.publishedTracks.audio }
              ]}
            />
          </>
        }
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
          </div>
        </DeviceModal>
      )}
      {isCatalogModalOpen && (
        <DeviceModal title="Catalogs" onClose={() => setIsCatalogModalOpen(false)} size="wide">
          <CatalogTrackEditor
            tracks={catalogTracks}
            videoEncodingOptions={videoEncodingOptions}
            audioEncodingOptions={audioEncodingOptions}
            onAddTrack={onAddCatalogTrack}
            onRemoveTrack={onRemoveCatalogTrack}
          />
        </DeviceModal>
      )}

      {remoteMembers.map((member) => {
        const media = remoteMedia.get(member.id)
        const cameraOverlay = renderStatsOverlay(
          'Received',
          {
            bitrate: media?.videoBitrateKbps,
            latencyRender: media?.videoLatencyRenderMs,
            latencyReceive: media?.videoLatencyReceiveMs
          },
          {
            bitrate: media?.audioBitrateKbps,
            latencyRender: media?.audioLatencyRenderMs,
            latencyReceive: media?.audioLatencyReceiveMs
          }
        )
        const screenShareOverlay = renderStatsOverlay(
          'Received',
          {
            bitrate: media?.screenShareBitrateKbps,
            latencyRender: media?.screenShareLatencyRenderMs,
            latencyReceive: media?.screenShareLatencyReceiveMs
          },
          undefined
        )
        const remotePrimaryVideoStream = media?.videoStream ?? media?.screenShareStream ?? null
        const remoteSecondaryVideoStream =
          media?.videoStream && media?.screenShareStream ? media.screenShareStream : null
        const remotePrimaryOverlay = media?.videoStream ? cameraOverlay : screenShareOverlay
        return (
          <MemberCard
            key={member.id}
            title={member.name}
            headerActions={
              <IconButton
                ariaLabel="Configure jitter buffer"
                title="Jitter buffer settings"
                onClick={() => setJitterModalTarget(member.id)}
              >
                <Settings2 className="h-4 w-4" />
              </IconButton>
            }
            videoStream={remotePrimaryVideoStream}
            videoOverlay={remotePrimaryOverlay}
            secondaryVideoStream={remoteSecondaryVideoStream}
            secondaryVideoOverlay={screenShareOverlay}
            secondaryVideoTitle={remoteSecondaryVideoStream ? 'Screen Share' : undefined}
            audioStream={media?.audioStream ?? null}
            placeholder="Awaiting video"
            secondaryPlaceholder="Awaiting screen share"
            details={
              <RemoteCatalogSubscribePanel
                tracks={remoteCatalogTracks.get(member.id) ?? []}
                selected={remoteCatalogSelections.get(member.id)}
                isLoading={catalogLoadingMemberIds.has(member.id)}
                isCatalogSubscribed={catalogSubscribedMemberIds.has(member.id)}
                isVideoSubscribed={member.subscribedTracks.video.isSubscribed}
                isVideoSubscribing={member.subscribedTracks.video.isSubscribing}
                isVideoUnsubscribing={catalogUnsubscribingTrackKeys.has(buildTrackActionKey(member.id, 'video'))}
                isScreenshareSubscribed={member.subscribedTracks.screenshare.isSubscribed}
                isScreenshareSubscribing={member.subscribedTracks.screenshare.isSubscribing}
                isScreenshareUnsubscribing={catalogUnsubscribingTrackKeys.has(
                  buildTrackActionKey(member.id, 'screenshare')
                )}
                isAudioSubscribed={member.subscribedTracks.audio.isSubscribed}
                isAudioSubscribing={member.subscribedTracks.audio.isSubscribing}
                isAudioUnsubscribing={catalogUnsubscribingTrackKeys.has(buildTrackActionKey(member.id, 'audio'))}
                onLoadCatalog={() => onLoadCatalogTracks(member.id)}
                onSelectTrack={(role, trackName) => onSelectCatalogTrack(member.id, role, trackName)}
                onSubscribeVideo={() => onSubscribeVideoTrack(member.id)}
                onSubscribeScreenshare={() => onSubscribeScreenshareTrack(member.id)}
                onSubscribeAudio={() => onSubscribeAudioTrack(member.id)}
                onUnsubscribeVideo={() => onUnsubscribeVideoTrack(member.id)}
                onUnsubscribeScreenshare={() => onUnsubscribeScreenshareTrack(member.id)}
                onUnsubscribeAudio={() => onUnsubscribeAudioTrack(member.id)}
              />
            }
          />
        )
      })}
      {jitterModalTarget && (
        <DeviceModal title="Jitter Buffer" onClose={() => setJitterModalTarget(null)}>
          <div className="space-y-3">
            <h4 className="text-base font-semibold text-white">
              Jitter Buffer ({findMemberName(remoteMembers, jitterModalTarget)})
            </h4>
            <VideoJitterBufferControls
              value={videoJitterConfigs.get(jitterModalTarget)}
              onChange={(config) => onChangeVideoJitterConfig(jitterModalTarget, config)}
            />
            <AudioJitterBufferControls
              value={audioJitterConfigs.get(jitterModalTarget)}
              onChange={(config) => onChangeAudioJitterConfig(jitterModalTarget, config)}
            />
          </div>
        </DeviceModal>
      )}
    </section>
  )
}

interface MemberCardProps {
  title: string
  headerActions?: ReactNode
  videoStream?: MediaStream | null
  videoOverlay?: ReactNode
  secondaryVideoStream?: MediaStream | null
  secondaryVideoOverlay?: ReactNode
  secondaryVideoTitle?: string
  audioStream?: MediaStream | null
  placeholder: string
  secondaryPlaceholder?: string
  muted?: boolean
  details: ReactNode
}

function MemberCard({
  title,
  headerActions,
  videoStream,
  videoOverlay,
  secondaryVideoStream,
  secondaryVideoOverlay,
  secondaryVideoTitle,
  audioStream,
  placeholder,
  secondaryPlaceholder = 'Video unavailable',
  muted = false,
  details
}: MemberCardProps) {
  return (
    <div className="rounded-2xl border border-white/10 bg-white/10 p-6 shadow-xl backdrop-blur">
      <div className="mb-3 flex items-center justify-between gap-3">
        <h3 className="text-xl font-semibold text-white">{title}</h3>
        {headerActions}
      </div>
      <MediaStreamVideo stream={videoStream} muted={muted} placeholder={placeholder} overlay={videoOverlay} />
      {secondaryVideoStream && (
        <div className="mt-2 space-y-1">
          {secondaryVideoTitle ? (
            <div className="text-xs font-semibold text-blue-200">{secondaryVideoTitle}</div>
          ) : null}
          <MediaStreamVideo
            stream={secondaryVideoStream}
            muted={muted}
            placeholder={secondaryPlaceholder}
            overlay={secondaryVideoOverlay}
          />
        </div>
      )}
      {audioStream && <MediaStreamAudio stream={audioStream} className="hidden" />}
      <div className="mt-4 space-y-3 text-sm text-blue-100">{details}</div>
    </div>
  )
}

interface ControlButtonProps {
  active: boolean
  disabled: boolean
  onClick: () => void
  activeLabel: string
  inactiveLabel: string
  IconOn: typeof Mic
  IconOff: typeof MicOff
}

function ControlButton({ active, disabled, onClick, activeLabel, inactiveLabel, IconOn, IconOff }: ControlButtonProps) {
  const Icon = active ? IconOn : IconOff
  return (
    <button
      onClick={onClick}
      disabled={disabled}
      className={`flex items-center gap-2 rounded-lg px-3 py-2 text-xs font-semibold transition ${
        active ? 'bg-blue-500 text-white hover:bg-blue-600' : 'bg-white/10 text-blue-100 hover:bg-white/20'
      } ${disabled ? 'cursor-not-allowed opacity-60' : ''}`}
    >
      <Icon className="h-4 w-4" />
      <span>{active ? activeLabel : inactiveLabel}</span>
    </button>
  )
}

function IconButton({
  ariaLabel,
  onClick,
  children,
  title
}: {
  ariaLabel: string
  onClick: () => void
  children: ReactNode
  title?: string
}) {
  return (
    <button
      type="button"
      aria-label={ariaLabel}
      title={title}
      onClick={onClick}
      className="rounded-lg bg-white/10 px-2 py-2 text-blue-100 transition hover:bg-white/20"
    >
      {children}
    </button>
  )
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

function findMemberName(members: RemoteMember[], id: string): string {
  return members.find((m) => m.id === id)?.name ?? id
}

function CatalogTrackEditor({
  tracks,
  videoEncodingOptions,
  audioEncodingOptions,
  onAddTrack,
  onRemoveTrack
}: {
  tracks: EditableCallCatalogTrack[]
  videoEncodingOptions: VideoEncodingOptionSet
  audioEncodingOptions: AudioEncodingOptionSet
  onAddTrack: (track: Omit<EditableCallCatalogTrack, 'id'>) => void
  onRemoveTrack: (id: string) => void
}) {
  const videoTracks = tracks.filter((track) => track.role === 'video' && !isScreenShareTrackName(track.name))
  const screenshareTracks = tracks.filter((track) => track.role === 'video' && isScreenShareTrackName(track.name))
  const audioTracks = tracks.filter((track) => track.role === 'audio')
  const [activeTab, setActiveTab] = useState<CatalogTabKey>('video')
  const [isAddingTrack, setIsAddingTrack] = useState(false)
  const [addTrackKind, setAddTrackKind] = useState<CatalogTabKey>('video')
  const [videoDraft, setVideoDraft] = useState(() => createVideoTrackDraft(videoEncodingOptions))
  const [screenshareDraft, setScreenshareDraft] = useState(() => createScreenShareTrackDraft(videoEncodingOptions))
  const [audioDraft, setAudioDraft] = useState(() => createAudioTrackDraft(audioEncodingOptions))

  const tabItems: Array<{
    key: CatalogTabKey
    title: string
    description: string
    tracks: EditableCallCatalogTrack[]
  }> = [
    {
      key: 'video',
      title: 'Video',
      description: 'Camera tracks',
      tracks: videoTracks
    },
    {
      key: 'screenshare',
      title: 'Screenshare',
      description: 'Display capture tracks',
      tracks: screenshareTracks
    },
    {
      key: 'audio',
      title: 'Audio',
      description: 'Microphone tracks',
      tracks: audioTracks
    }
  ]
  const currentTab = tabItems.find((tab) => tab.key === activeTab) ?? tabItems[0]
  const currentDraft =
    addTrackKind === 'video' ? videoDraft : addTrackKind === 'screenshare' ? screenshareDraft : audioDraft

  const setCurrentDraft = (patch: Partial<Omit<EditableCallCatalogTrack, 'id'>>) => {
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

  const addDraftTrack = () => {
    const normalizedName = (currentDraft.name || '').trim()
    if (!normalizedName) {
      return
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
            <label className="flex flex-col gap-1 text-xs text-blue-100">
              <span>Track Type</span>
              <select
                className="rounded border border-white/10 bg-white/10 px-2 py-1 text-sm text-white"
                value={addTrackKind}
                onChange={(event) => setAddTrackKind(event.target.value as CatalogTabKey)}
              >
                <option value="video" className="bg-slate-900 text-white">
                  Video
                </option>
                <option value="screenshare" className="bg-slate-900 text-white">
                  Screenshare
                </option>
                <option value="audio" className="bg-slate-900 text-white">
                  Audio
                </option>
              </select>
            </label>
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
                <label className="flex flex-col gap-1 text-xs text-blue-100">
                  <span>Codec Preset</span>
                  <select
                    className="rounded border border-white/10 bg-white/10 px-2 py-1 text-sm text-white"
                    value={findVideoCodecOptionId(videoEncodingOptions, currentDraft.codec)}
                    onChange={(event) => {
                      const option = videoEncodingOptions.codecOptions.find((entry) => entry.id === event.target.value)
                      if (option) {
                        setCurrentDraft({ codec: option.codec })
                      }
                    }}
                  >
                    {videoEncodingOptions.codecOptions.map((option) => (
                      <option key={option.id} value={option.id} className="bg-slate-900 text-white">
                        {option.label}
                      </option>
                    ))}
                  </select>
                </label>
                <label className="flex flex-col gap-1 text-xs text-blue-100">
                  <span>Resolution Preset</span>
                  <select
                    className="rounded border border-white/10 bg-white/10 px-2 py-1 text-sm text-white"
                    value={findVideoResolutionOptionId(videoEncodingOptions, currentDraft.width, currentDraft.height)}
                    onChange={(event) => {
                      const option = videoEncodingOptions.resolutionOptions.find(
                        (entry) => entry.id === event.target.value
                      )
                      if (option) {
                        setCurrentDraft({ width: option.width, height: option.height })
                      }
                    }}
                  >
                    {videoEncodingOptions.resolutionOptions.map((option) => (
                      <option key={option.id} value={option.id} className="bg-slate-900 text-white">
                        {option.label}
                      </option>
                    ))}
                  </select>
                </label>
                <label className="col-span-full flex flex-col gap-1 text-xs text-blue-100">
                  <span>Bitrate Preset</span>
                  <select
                    className="rounded border border-white/10 bg-white/10 px-2 py-1 text-sm text-white"
                    value={findVideoBitrateOptionId(videoEncodingOptions, currentDraft.bitrate)}
                    onChange={(event) => {
                      const option = videoEncodingOptions.bitrateOptions.find(
                        (entry) => entry.id === event.target.value
                      )
                      if (option) {
                        setCurrentDraft({ bitrate: option.bitrate })
                      }
                    }}
                  >
                    {videoEncodingOptions.bitrateOptions.map((option) => (
                      <option key={option.id} value={option.id} className="bg-slate-900 text-white">
                        {option.label}
                      </option>
                    ))}
                  </select>
                </label>
              </>
            ) : (
              <>
                <label className="flex flex-col gap-1 text-xs text-blue-100">
                  <span>Codec Preset</span>
                  <select
                    className="rounded border border-white/10 bg-white/10 px-2 py-1 text-sm text-white"
                    value={findAudioCodecOptionId(audioEncodingOptions, currentDraft.codec)}
                    onChange={(event) => {
                      const option = audioEncodingOptions.codecOptions.find((entry) => entry.id === event.target.value)
                      if (option) {
                        setCurrentDraft({ codec: option.codec })
                      }
                    }}
                  >
                    {audioEncodingOptions.codecOptions.map((option) => (
                      <option key={option.id} value={option.id} className="bg-slate-900 text-white">
                        {option.label}
                      </option>
                    ))}
                  </select>
                </label>
                <label className="flex flex-col gap-1 text-xs text-blue-100">
                  <span>Bitrate Preset</span>
                  <select
                    className="rounded border border-white/10 bg-white/10 px-2 py-1 text-sm text-white"
                    value={findAudioBitrateOptionId(audioEncodingOptions, currentDraft.bitrate)}
                    onChange={(event) => {
                      const option = audioEncodingOptions.bitrateOptions.find(
                        (entry) => entry.id === event.target.value
                      )
                      if (option) {
                        setCurrentDraft({ bitrate: option.bitrate })
                      }
                    }}
                  >
                    {audioEncodingOptions.bitrateOptions.map((option) => (
                      <option key={option.id} value={option.id} className="bg-slate-900 text-white">
                        {option.label}
                      </option>
                    ))}
                  </select>
                </label>
                <label className="col-span-full flex flex-col gap-1 text-xs text-blue-100">
                  <span>Channel Preset</span>
                  <select
                    className="rounded border border-white/10 bg-white/10 px-2 py-1 text-sm text-white"
                    value={findAudioChannelOptionId(audioEncodingOptions, currentDraft.channelConfig)}
                    onChange={(event) => {
                      const option = audioEncodingOptions.channelOptions.find(
                        (entry) => entry.id === event.target.value
                      )
                      if (option) {
                        setCurrentDraft({
                          channelConfig: channelConfigForChannels(option.channels),
                          samplerate: currentDraft.samplerate ?? 48_000
                        })
                      }
                    }}
                  >
                    {audioEncodingOptions.channelOptions.map((option) => (
                      <option key={option.id} value={option.id} className="bg-slate-900 text-white">
                        {option.label}
                      </option>
                    ))}
                  </select>
                </label>
              </>
            )}
          </div>

          <div className="flex justify-start gap-2">
            <button
              type="button"
              className="rounded bg-blue-600 px-3 py-1.5 text-xs font-semibold text-white hover:bg-blue-700"
              onClick={addDraftTrack}
            >
              Add to Catalog
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

function findVideoCodecOptionId(options: VideoEncodingOptionSet, codec: string | undefined): string {
  return options.codecOptions.find((entry) => entry.codec === codec)?.id ?? options.codecOptions[0]?.id ?? ''
}

function createVideoTrackDraft(options: VideoEncodingOptionSet): Omit<EditableCallCatalogTrack, 'id'> {
  const codec = options.codecOptions[0]?.codec ?? 'avc1.42E01E'
  const resolution = options.resolutionOptions[0]
  const bitrate = options.bitrateOptions[0]?.bitrate ?? 800_000
  return {
    name: `camera_${Date.now()}`,
    label: 'Camera',
    role: 'video',
    codec,
    width: resolution?.width ?? 1280,
    height: resolution?.height ?? 720,
    bitrate,
    samplerate: undefined,
    channelConfig: undefined,
    isLive: true
  }
}

function createScreenShareTrackDraft(options: VideoEncodingOptionSet): Omit<EditableCallCatalogTrack, 'id'> {
  const codec = options.codecOptions[0]?.codec ?? 'av01.0.08M.08'
  const resolution = options.resolutionOptions[0]
  const bitrate = options.bitrateOptions[0]?.bitrate ?? 1_000_000
  return {
    name: `screenshare_${Date.now()}`,
    label: 'Screenshare',
    role: 'video',
    codec,
    width: resolution?.width ?? 1920,
    height: resolution?.height ?? 1080,
    bitrate,
    samplerate: undefined,
    channelConfig: undefined,
    isLive: true
  }
}

function createAudioTrackDraft(options: AudioEncodingOptionSet): Omit<EditableCallCatalogTrack, 'id'> {
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
    samplerate: 48_000,
    channelConfig: channelConfigForChannels(channels),
    isLive: true
  }
}

function findVideoResolutionOptionId(
  options: VideoEncodingOptionSet,
  width: number | undefined,
  height: number | undefined
): string {
  return (
    options.resolutionOptions.find((entry) => entry.width === width && entry.height === height)?.id ??
    options.resolutionOptions[0]?.id ??
    ''
  )
}

function findVideoBitrateOptionId(options: VideoEncodingOptionSet, bitrate: number | undefined): string {
  return options.bitrateOptions.find((entry) => entry.bitrate === bitrate)?.id ?? options.bitrateOptions[0]?.id ?? ''
}

function findAudioCodecOptionId(options: AudioEncodingOptionSet, codec: string | undefined): string {
  return options.codecOptions.find((entry) => entry.codec === codec)?.id ?? options.codecOptions[0]?.id ?? ''
}

function findAudioBitrateOptionId(options: AudioEncodingOptionSet, bitrate: number | undefined): string {
  return options.bitrateOptions.find((entry) => entry.bitrate === bitrate)?.id ?? options.bitrateOptions[0]?.id ?? ''
}

function findAudioChannelOptionId(options: AudioEncodingOptionSet, channelConfig: string | undefined): string {
  const normalized = channelConfig?.toLowerCase() ?? ''
  if (normalized.includes('mono')) {
    return options.channelOptions.find((entry) => entry.channels === 1)?.id ?? options.channelOptions[0]?.id ?? ''
  }
  if (normalized.includes('stereo')) {
    return options.channelOptions.find((entry) => entry.channels === 2)?.id ?? options.channelOptions[0]?.id ?? ''
  }
  return options.channelOptions[0]?.id ?? ''
}

function channelConfigForChannels(channels: number): string {
  if (channels <= 1) {
    return 'mono'
  }
  if (channels === 2) {
    return 'stereo'
  }
  return `${channels}ch`
}

function RemoteCatalogSubscribePanel({
  tracks,
  selected,
  isLoading,
  isCatalogSubscribed,
  isVideoSubscribed,
  isVideoSubscribing,
  isVideoUnsubscribing,
  isScreenshareSubscribed,
  isScreenshareSubscribing,
  isScreenshareUnsubscribing,
  isAudioSubscribed,
  isAudioSubscribing,
  isAudioUnsubscribing,
  onLoadCatalog,
  onSelectTrack,
  onSubscribeVideo,
  onSubscribeScreenshare,
  onSubscribeAudio,
  onUnsubscribeVideo,
  onUnsubscribeScreenshare,
  onUnsubscribeAudio
}: {
  tracks: CallCatalogTrack[]
  selected?: { video?: string; screenshare?: string; audio?: string }
  isLoading: boolean
  isCatalogSubscribed: boolean
  isVideoSubscribed: boolean
  isVideoSubscribing: boolean
  isVideoUnsubscribing: boolean
  isScreenshareSubscribed: boolean
  isScreenshareSubscribing: boolean
  isScreenshareUnsubscribing: boolean
  isAudioSubscribed: boolean
  isAudioSubscribing: boolean
  isAudioUnsubscribing: boolean
  onLoadCatalog: () => void
  onSelectTrack: (role: CatalogSubscribeRole, trackName: string) => void
  onSubscribeVideo: () => void
  onSubscribeScreenshare: () => void
  onSubscribeAudio: () => void
  onUnsubscribeVideo: () => void
  onUnsubscribeScreenshare: () => void
  onUnsubscribeAudio: () => void
}) {
  const videoTracks = tracks.filter((track) => track.role === 'video' && !isScreenShareTrackName(track.name))
  const screenshareTracks = tracks.filter((track) => track.role === 'video' && isScreenShareTrackName(track.name))
  const audioTracks = tracks.filter((track) => track.role === 'audio')
  const selectedVideo = selected?.video ?? videoTracks[0]?.name ?? ''
  const selectedScreenshare = selected?.screenshare ?? screenshareTracks[0]?.name ?? ''
  const selectedAudio = selected?.audio ?? audioTracks[0]?.name ?? ''
  const hasCatalog = isCatalogSubscribed
  const trackRows: {
    role: CatalogSubscribeRole
    title: string
    tracks: CallCatalogTrack[]
    selectedTrack: string
    subscribed: boolean
    subscribing: boolean
    unsubscribing: boolean
    onSubscribe: () => void
    onUnsubscribe: () => void
  }[] = [
    {
      role: 'video',
      title: 'Video Track',
      tracks: videoTracks,
      selectedTrack: selectedVideo,
      subscribed: isVideoSubscribed,
      subscribing: isVideoSubscribing,
      unsubscribing: isVideoUnsubscribing,
      onSubscribe: onSubscribeVideo,
      onUnsubscribe: onUnsubscribeVideo
    },
    {
      role: 'screenshare',
      title: 'Screenshare Track',
      tracks: screenshareTracks,
      selectedTrack: selectedScreenshare,
      subscribed: isScreenshareSubscribed,
      subscribing: isScreenshareSubscribing,
      unsubscribing: isScreenshareUnsubscribing,
      onSubscribe: onSubscribeScreenshare,
      onUnsubscribe: onUnsubscribeScreenshare
    },
    {
      role: 'audio',
      title: 'Audio Track',
      tracks: audioTracks,
      selectedTrack: selectedAudio,
      subscribed: isAudioSubscribed,
      subscribing: isAudioSubscribing,
      unsubscribing: isAudioUnsubscribing,
      onSubscribe: onSubscribeAudio,
      onUnsubscribe: onUnsubscribeAudio
    }
  ]
  const catalogButtonLabel = isLoading ? 'Loading...' : 'Catalog Subscribe'

  return (
    <div className="space-y-2 rounded-md border border-white/10 bg-white/5 p-3">
      <div className="flex justify-start">
        <button
          type="button"
          disabled={hasCatalog || isLoading}
          onClick={onLoadCatalog}
          className={`rounded px-3 py-1 text-xs font-semibold text-white transition ${
            hasCatalog ? 'bg-slate-500/70' : 'bg-blue-600 hover:bg-blue-700'
          } ${isLoading ? 'cursor-wait opacity-70' : ''} ${hasCatalog ? 'cursor-not-allowed opacity-70' : ''}`}
        >
          {catalogButtonLabel}
        </button>
      </div>
      {trackRows.map((row) => {
        const busy = row.subscribing || row.unsubscribing
        const canSubscribe = hasCatalog && Boolean(row.selectedTrack) && !row.subscribed && !busy
        const canUnsubscribe = hasCatalog && row.subscribed && !busy
        return (
          <TrackSubscribeRow
            key={row.role}
            title={row.title}
            tracks={row.tracks}
            selected={row.selectedTrack}
            disabled={!hasCatalog || row.subscribed || busy}
            statusText={formatSubscribeStatus(row.subscribed, row.subscribing, row.unsubscribing)}
            subscribeLabel={row.subscribing ? 'Subscribing...' : 'Subscribe'}
            unsubscribeLabel={row.unsubscribing ? 'Unsubscribing...' : 'Unsubscribe'}
            canSubscribe={canSubscribe}
            canUnsubscribe={canUnsubscribe}
            busy={busy}
            onChange={(trackName) => onSelectTrack(row.role, trackName)}
            onSubscribe={row.onSubscribe}
            onUnsubscribe={row.onUnsubscribe}
          />
        )
      })}
    </div>
  )
}

function TrackSubscribeRow({
  title,
  tracks,
  selected,
  disabled,
  statusText,
  subscribeLabel,
  unsubscribeLabel,
  canSubscribe,
  canUnsubscribe,
  busy,
  onChange,
  onSubscribe,
  onUnsubscribe
}: {
  title: string
  tracks: CallCatalogTrack[]
  selected: string
  disabled: boolean
  statusText: string
  subscribeLabel: string
  unsubscribeLabel: string
  canSubscribe: boolean
  canUnsubscribe: boolean
  busy: boolean
  onChange: (trackName: string) => void
  onSubscribe: () => void
  onUnsubscribe: () => void
}) {
  return (
    <div className="space-y-1 rounded-md border border-white/10 bg-white/[0.03] p-2">
      <div className="flex items-center justify-between gap-2">
        <span className="text-xs font-semibold text-blue-100">{title}</span>
        <span className="text-[11px] text-blue-300">{statusText}</span>
      </div>
      <div className="grid gap-2 md:grid-cols-[1fr_auto_auto]">
        <select
          value={selected}
          disabled={disabled}
          onChange={(event) => onChange(event.target.value)}
          className="rounded border border-white/10 bg-white/10 px-2 py-1 text-sm text-white outline-none focus:border-blue-300 focus:ring-1 focus:ring-blue-300 disabled:opacity-60"
        >
          {tracks.length === 0 ? (
            <option value="" className="bg-slate-900 text-white">
              No tracks
            </option>
          ) : (
            tracks.map((track) => (
              <option key={track.name} value={track.name} className="bg-slate-900 text-white">
                {formatCatalogTrackLabel(track)}
              </option>
            ))
          )}
        </select>
        <button
          type="button"
          disabled={!canSubscribe}
          onClick={onSubscribe}
          className={`rounded px-3 py-1 text-xs font-semibold text-white transition ${
            canSubscribe ? 'bg-emerald-600 hover:bg-emerald-700' : 'bg-slate-500/70'
          } ${busy ? 'cursor-wait opacity-70' : ''} ${!canSubscribe ? 'cursor-not-allowed opacity-70' : ''}`}
        >
          {subscribeLabel}
        </button>
        <button
          type="button"
          disabled={!canUnsubscribe}
          onClick={onUnsubscribe}
          className={`rounded px-3 py-1 text-xs font-semibold text-white transition ${
            canUnsubscribe ? 'bg-amber-600 hover:bg-amber-700' : 'bg-slate-500/70'
          } ${busy ? 'cursor-wait opacity-70' : ''} ${!canUnsubscribe ? 'cursor-not-allowed opacity-70' : ''}`}
        >
          {unsubscribeLabel}
        </button>
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

function buildTrackActionKey(memberId: string, role: CatalogSubscribeRole): string {
  return `${memberId}:${role}`
}

function formatCatalogTrackLabel(track: CallCatalogTrack): string {
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
  track: CallCatalogTrack
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
  if (track.role === 'audio' && typeof track.samplerate === 'number') {
    entries.push({ key: 'samplerate', label: 'Sample Rate', value: `${track.samplerate}Hz` })
  }
  if (track.role === 'audio' && track.channelConfig) {
    entries.push({ key: 'channel', label: 'Channel', value: track.channelConfig })
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

function TrackList({ title, items }: { title: string; items: { label: string; enabled: boolean }[] }) {
  return (
    <div>
      <span className="font-medium text-blue-200">{title}:</span>
      <ul className="mt-1 space-y-1">
        {items.map((item) => (
          <li key={item.label}>
            {item.label}: {item.enabled ? 'Enabled' : 'Disabled'}
          </li>
        ))}
      </ul>
    </div>
  )
}

type TrackStats = {
  bitrate?: number | null
  latencyRender?: number | null
  latencyReceive?: number | null
}

function renderStatsOverlay(label: string, video?: TrackStats, audio?: TrackStats): ReactNode | undefined {
  const rows: string[] = []

  const formatBitrate = (kbps?: number | null) => {
    if (typeof kbps !== 'number') {
      return null
    }
    return `${kbps.toFixed(0)} kbps`
  }

  const formatLatencyPair = (renderMs?: number | null, receiveMs?: number | null) => {
    const renderText = typeof renderMs === 'number' ? `render ${renderMs.toFixed(0)} ms` : null
    const receiveText = typeof receiveMs === 'number' ? `recv ${receiveMs.toFixed(0)} ms` : null
    const parts = [renderText, receiveText].filter(Boolean)
    return parts.length ? parts.join(' / ') : null
  }

  const videoParts: string[] = []
  const videoBitrate = formatBitrate(video?.bitrate)
  if (videoBitrate) {
    videoParts.push(videoBitrate)
  }
  const videoLatency = formatLatencyPair(video?.latencyRender, video?.latencyReceive)
  if (videoLatency) {
    videoParts.push(videoLatency)
  }
  if (videoParts.length) {
    rows.push(`${label} Video: ${videoParts.join(' / ')}`)
  }

  const audioParts: string[] = []
  const audioBitrate = formatBitrate(audio?.bitrate)
  if (audioBitrate) {
    audioParts.push(audioBitrate)
  }
  const audioLatency = formatLatencyPair(audio?.latencyRender, audio?.latencyReceive)
  if (audioLatency) {
    audioParts.push(audioLatency)
  }
  if (audioParts.length) {
    rows.push(`${label} Audio: ${audioParts.join(' / ')}`)
  }

  if (!rows.length) {
    return undefined
  }

  return (
    <div className="flex flex-col text-right leading-tight">
      {rows.map((text) => (
        <span key={text}>{text}</span>
      ))}
    </div>
  )
}
