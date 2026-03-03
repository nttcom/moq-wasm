import { LocalMember, RemoteMember } from '../types/member'
import { RemoteMediaStreams } from '../types/media'
import { MediaStreamVideo, MediaStreamAudio } from './MediaStreamElements'
import { ReactNode, useEffect, useState } from 'react'
import { DEFAULT_VIDEO_JITTER_CONFIG, type VideoJitterConfig, type AudioJitterConfig } from '../types/jitterBuffer'
import { VideoJitterBufferControls, AudioJitterBufferControls } from './JitterBufferControls'
import { BarChart3, LayoutGrid, Mic, MicOff, Minus, Monitor, Plus, Settings2, Video, VideoOff } from 'lucide-react'
import { DeviceSelector } from './DeviceSelector'
import type { CaptureSettingsState } from '../types/captureConstraints'
import { GetUserMediaForm } from './GetUserMediaForm'
import type { CallCatalogTrack, CatalogSubscribeRole, EditableCallCatalogTrack } from '../types/catalog'
import { DEFAULT_AUDIO_STREAM_UPDATE_SETTINGS, DEFAULT_VIDEO_KEYFRAME_INTERVAL } from '../types/catalog'
import { isScreenShareTrackName } from '../utils/catalogTrackName'
import type { SidebarStatsSample } from '../types/stats'
import { MemberStatsCharts } from './MemberStatsCharts'
import { JitterBufferVisualizer } from './JitterBufferVisualizer'
import type {
  VideoCodecOption,
  VideoHardwareAccelerationOption,
  VideoResolutionOption,
  VideoEncodingSettings
} from '../types/videoEncoding'
import type { SubscribedCatalogTrack } from '../media/mediaPublisher'

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

interface MemberGridProps {
  localMember: LocalMember
  remoteMembers: RemoteMember[]
  localVideoStream?: MediaStream | null
  localScreenShareStream?: MediaStream | null
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
  videoHardwareAccelerationOptions: VideoHardwareAccelerationOption[]
  selectedVideoEncoding: VideoEncodingSettings
  onSelectVideoEncoding: (settings: Partial<VideoEncodingSettings>) => Promise<void>
  selectedScreenShareEncoding: VideoEncodingSettings
  onSelectScreenShareEncoding: (settings: Partial<VideoEncodingSettings>) => Promise<void>
  audioEncodingOptions: AudioEncodingOptionSet
  captureSettings: CaptureSettingsState
  onChangeCaptureSettings: (settings: Partial<CaptureSettingsState>) => void
  onApplyCaptureSettings: () => void
  catalogTracks: EditableCallCatalogTrack[]
  subscribedCatalogTracks: SubscribedCatalogTrack[]
  onAddCatalogTrack: (track: Omit<EditableCallCatalogTrack, 'id'>) => void
  onUpdateCatalogTrack: (id: string, patch: Partial<EditableCallCatalogTrack>) => void
  onRemoveCatalogTrack: (id: string) => void
  remoteCatalogTracks: Map<string, CallCatalogTrack[]>
  remoteCatalogSelections: Map<string, { video?: string; screenshare?: string; audio?: string; chat?: string }>
  catalogLoadingMemberIds: Set<string>
  catalogSubscribedMemberIds: Set<string>
  catalogUnsubscribingTrackKeys: Set<string>
  onSelectCatalogTrack: (memberId: string, role: CatalogSubscribeRole, trackName: string) => void
  onSubscribeVideoTrack: (memberId: string) => void
  onSubscribeScreenshareTrack: (memberId: string) => void
  onSubscribeAudioTrack: (memberId: string) => void
  onSubscribeChatTrack: (memberId: string) => void
  onUnsubscribeVideoTrack: (memberId: string) => void
  onUnsubscribeScreenshareTrack: (memberId: string) => void
  onUnsubscribeAudioTrack: (memberId: string) => void
  onUnsubscribeChatTrack: (memberId: string) => void
  statsHistory: Map<string, SidebarStatsSample[]>
}

export function MemberGrid({
  localMember,
  remoteMembers,
  localVideoStream,
  localScreenShareStream,
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
  catalogLoadingMemberIds,
  catalogSubscribedMemberIds,
  catalogUnsubscribingTrackKeys,
  onSelectCatalogTrack,
  onSubscribeVideoTrack,
  onSubscribeScreenshareTrack,
  onSubscribeAudioTrack,
  onSubscribeChatTrack,
  onUnsubscribeVideoTrack,
  onUnsubscribeScreenshareTrack,
  onUnsubscribeAudioTrack,
  onUnsubscribeChatTrack,
  statsHistory
}: MemberGridProps) {
  const [isDeviceModalOpen, setIsDeviceModalOpen] = useState(false)
  const [isCatalogModalOpen, setIsCatalogModalOpen] = useState(false)
  const [jitterModalTarget, setJitterModalTarget] = useState<string | null>(null)
  const [statsModalTarget, setStatsModalTarget] = useState<string | null>(null)
  const [visualizedMemberIds, setVisualizedMemberIds] = useState<Set<string>>(new Set())

  const localPrimaryVideoStream = localVideoStream ?? localScreenShareStream ?? null
  const localSecondaryVideoStream = localVideoStream && localScreenShareStream ? localScreenShareStream : null

  useEffect(() => {
    const remoteIds = new Set(remoteMembers.map((member) => member.id))
    setVisualizedMemberIds((prev) => {
      let changed = false
      const next = new Set<string>()
      for (const memberId of prev) {
        if (remoteIds.has(memberId)) {
          next.add(memberId)
        } else {
          changed = true
        }
      }
      return changed ? next : prev
    })
  }, [remoteMembers])

  const toggleMemberVisualization = (memberId: string) => {
    setVisualizedMemberIds((prev) => {
      const next = new Set(prev)
      if (next.has(memberId)) {
        next.delete(memberId)
      } else {
        next.add(memberId)
      }
      return next
    })
  }

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
              activeLabel="Turn camera off"
              inactiveLabel="Turn camera on"
              IconOn={Video}
              IconOff={VideoOff}
            />
            <ControlButton
              active={screenShareEnabled}
              disabled={cameraBusy}
              onClick={onToggleScreenShare}
              activeLabel="Stop screen share"
              inactiveLabel="Start screen share"
              IconOn={Monitor}
              IconOff={Monitor}
            />
            <ControlButton
              active={microphoneEnabled}
              disabled={microphoneBusy}
              onClick={onToggleMicrophone}
              activeLabel="Turn microphone off"
              inactiveLabel="Turn microphone on"
              IconOn={Mic}
              IconOff={MicOff}
            />
            <IconButton ariaLabel="Select devices" onClick={() => setIsDeviceModalOpen(true)} title="Device settings">
              <Settings2 className="h-4 w-4" />
            </IconButton>
            <IconButton ariaLabel="Show stats" title="Show stats" onClick={() => setStatsModalTarget(localMember.id)}>
              <BarChart3 className="h-4 w-4" />
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
        secondaryVideoStream={localSecondaryVideoStream}
        secondaryVideoTitle={localSecondaryVideoStream ? 'Screen Share' : undefined}
        audioStream={null}
        muted
        placeholder="Camera disabled"
        secondaryPlaceholder="Screen share disabled"
        details={<CatalogsPanel tracks={catalogTracks} subscribedTracks={subscribedCatalogTracks} />}
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
              <label className="flex items-center justify-between gap-3 text-sm text-blue-50">
                <span>Camera</span>
                <select
                  value={selectedVideoEncoding.hardwareAcceleration}
                  onChange={(event) =>
                    void onSelectVideoEncoding({
                      hardwareAcceleration: event.target.value as HardwareAcceleration
                    })
                  }
                  className="w-44 rounded-md bg-white/10 px-2 py-1 text-sm text-white outline-none ring-1 ring-white/10 focus:ring-blue-400"
                >
                  {videoHardwareAccelerationOptions.map((option) => (
                    <option key={option.id} value={option.value}>
                      {option.label}
                    </option>
                  ))}
                </select>
              </label>
              <label className="flex items-center justify-between gap-3 text-sm text-blue-50">
                <span>Screen share</span>
                <select
                  value={selectedScreenShareEncoding.hardwareAcceleration}
                  onChange={(event) =>
                    void onSelectScreenShareEncoding({
                      hardwareAcceleration: event.target.value as HardwareAcceleration
                    })
                  }
                  className="w-44 rounded-md bg-white/10 px-2 py-1 text-sm text-white outline-none ring-1 ring-white/10 focus:ring-1 focus:ring-blue-400"
                >
                  {videoHardwareAccelerationOptions.map((option) => (
                    <option key={option.id} value={option.value}>
                      {option.label}
                    </option>
                  ))}
                </select>
              </label>
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
      {remoteMembers.map((member) => {
        const media = remoteMedia.get(member.id)
        const isVisualizationEnabled = visualizedMemberIds.has(member.id)
        const videoJitterConfig = videoJitterConfigs.get(member.id) ?? DEFAULT_VIDEO_JITTER_CONFIG
        const remotePrimaryVideoStream = media?.videoStream ?? media?.screenShareStream ?? null
        const primaryVideoSource: 'camera' | 'screenshare' = media?.videoStream ? 'camera' : 'screenshare'
        const primaryVideoJitterBuffer =
          media?.videoStream && media.videoJitterBuffer ? media.videoJitterBuffer : media?.screenShareJitterBuffer
        const remoteSecondaryVideoStream =
          media?.videoStream && media?.screenShareStream ? media.screenShareStream : null
        return (
          <MemberCard
            key={member.id}
            title={member.name}
            headerActions={
              <div className="flex items-center gap-2">
                <IconButton
                  ariaLabel="Configure jitter buffer"
                  title="Jitter buffer settings"
                  onClick={() => setJitterModalTarget(member.id)}
                >
                  <Settings2 className="h-4 w-4" />
                </IconButton>
                <IconButton ariaLabel="Show stats" title="Show stats" onClick={() => setStatsModalTarget(member.id)}>
                  <BarChart3 className="h-4 w-4" />
                </IconButton>
                <IconButton
                  ariaLabel={
                    isVisualizationEnabled
                      ? 'Hide receive jitter buffer visualization'
                      : 'Show receive jitter buffer visualization'
                  }
                  title={
                    isVisualizationEnabled
                      ? 'Hide receive jitter buffer visualization'
                      : 'Show receive jitter buffer visualization'
                  }
                  onClick={() => toggleMemberVisualization(member.id)}
                  className={`rounded-lg px-2 py-2 transition ${
                    isVisualizationEnabled
                      ? 'bg-emerald-600 text-white hover:bg-emerald-700'
                      : 'bg-white/10 text-blue-100 hover:bg-white/20'
                  }`}
                >
                  <LayoutGrid className="h-4 w-4" />
                </IconButton>
              </div>
            }
            videoStream={remotePrimaryVideoStream}
            videoFooter={
              isVisualizationEnabled ? (
                <JitterBufferVisualizer
                  videoBuffer={primaryVideoJitterBuffer}
                  audioBuffer={media?.audioJitterBuffer}
                  videoDiagnostics={{
                    targetLatencyMs: videoJitterConfig?.pacing.targetLatencyMs,
                    networkLatencyMs:
                      primaryVideoSource === 'camera'
                        ? media?.videoLatencyReceiveMs
                        : media?.screenShareLatencyReceiveMs,
                    e2eLatencyMs:
                      primaryVideoSource === 'camera' ? media?.videoLatencyRenderMs : media?.screenShareLatencyRenderMs,
                    receiveToDecodeMs:
                      primaryVideoSource === 'camera'
                        ? media?.videoReceiveToDecodeMs
                        : media?.screenShareReceiveToDecodeMs,
                    receiveToRenderMs:
                      primaryVideoSource === 'camera'
                        ? media?.videoReceiveToRenderMs
                        : media?.screenShareReceiveToRenderMs,
                    pacingPreset: videoJitterConfig?.pacing.preset,
                    pacingPipeline: videoJitterConfig?.pacing.pipeline,
                    pacingEffectiveIntervalMs:
                      primaryVideoSource === 'camera'
                        ? media?.videoPacingEffectiveIntervalMs
                        : media?.screenSharePacingEffectiveIntervalMs,
                    pacingBufferedFrames:
                      primaryVideoSource === 'camera'
                        ? media?.videoPacingBufferedFrames
                        : media?.screenSharePacingBufferedFrames,
                    pacingTargetFrames:
                      primaryVideoSource === 'camera'
                        ? media?.videoPacingTargetFrames
                        : media?.screenSharePacingTargetFrames,
                    decodingGroupId:
                      primaryVideoSource === 'camera' ? media?.videoDecodingGroupId : media?.screenShareDecodingGroupId,
                    decodingObjectId:
                      primaryVideoSource === 'camera'
                        ? media?.videoDecodingObjectId
                        : media?.screenShareDecodingObjectId,
                    decoderCodec: primaryVideoSource === 'camera' ? media?.videoCodec : media?.screenShareCodec,
                    decoderWidth: primaryVideoSource === 'camera' ? media?.videoWidth : media?.screenShareWidth,
                    decoderHeight: primaryVideoSource === 'camera' ? media?.videoHeight : media?.screenShareHeight,
                    decoderAvcFormat:
                      primaryVideoSource === 'camera'
                        ? media?.videoDecoderAvcFormat
                        : media?.screenShareDecoderAvcFormat,
                    decoderDescriptionBytes:
                      primaryVideoSource === 'camera'
                        ? media?.videoDecoderDescriptionLength
                        : media?.screenShareDecoderDescriptionLength,
                    decoderHardwareAcceleration:
                      primaryVideoSource === 'camera'
                        ? media?.videoDecoderHardwareAcceleration
                        : media?.screenShareDecoderHardwareAcceleration,
                    decoderOptimizeForLatency:
                      primaryVideoSource === 'camera'
                        ? media?.videoDecoderOptimizeForLatency
                        : media?.screenShareDecoderOptimizeForLatency
                  }}
                />
              ) : undefined
            }
            secondaryVideoStream={remoteSecondaryVideoStream}
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
                isChatSubscribed={member.subscribedTracks.chat.isSubscribed}
                isChatSubscribing={member.subscribedTracks.chat.isSubscribing}
                isChatUnsubscribing={catalogUnsubscribingTrackKeys.has(buildTrackActionKey(member.id, 'chat'))}
                onSelectTrack={(role, trackName) => onSelectCatalogTrack(member.id, role, trackName)}
                onSubscribeVideo={() => onSubscribeVideoTrack(member.id)}
                onSubscribeScreenshare={() => onSubscribeScreenshareTrack(member.id)}
                onSubscribeAudio={() => onSubscribeAudioTrack(member.id)}
                onSubscribeChat={() => onSubscribeChatTrack(member.id)}
                onUnsubscribeVideo={() => onUnsubscribeVideoTrack(member.id)}
                onUnsubscribeScreenshare={() => onUnsubscribeScreenshareTrack(member.id)}
                onUnsubscribeAudio={() => onUnsubscribeAudioTrack(member.id)}
                onUnsubscribeChat={() => onUnsubscribeChatTrack(member.id)}
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
      {statsModalTarget && (
        <DeviceModal
          title={`Stats (${statsModalTarget === localMember.id ? localMember.name : findMemberName(remoteMembers, statsModalTarget)})`}
          onClose={() => setStatsModalTarget(null)}
          size="wide"
        >
          <div className="space-y-4">
            <MemberStatsCharts samples={statsHistory.get(statsModalTarget) ?? []} chartHeightClass="h-64" />
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
  videoFooter?: ReactNode
  secondaryVideoStream?: MediaStream | null
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
  videoFooter,
  secondaryVideoStream,
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
      <MediaStreamVideo stream={videoStream} muted={muted} placeholder={placeholder} footer={videoFooter} />
      {secondaryVideoStream && (
        <div className="mt-2 space-y-1">
          {secondaryVideoTitle ? (
            <div className="text-xs font-semibold text-blue-200">{secondaryVideoTitle}</div>
          ) : null}
          <MediaStreamVideo stream={secondaryVideoStream} muted={muted} placeholder={secondaryPlaceholder} />
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
  const label = active ? activeLabel : inactiveLabel
  return (
    <button
      type="button"
      aria-label={label}
      title={label}
      onClick={onClick}
      disabled={disabled}
      className={`inline-flex h-9 w-9 items-center justify-center rounded-lg p-2 transition ${
        active ? 'bg-blue-500 text-white hover:bg-blue-600' : 'bg-white/10 text-blue-100 hover:bg-white/20'
      } ${disabled ? 'cursor-not-allowed opacity-60' : ''}`}
    >
      <Icon className="h-4 w-4" />
    </button>
  )
}

function IconButton({
  ariaLabel,
  onClick,
  children,
  title,
  className
}: {
  ariaLabel: string
  onClick: () => void
  children: ReactNode
  title?: string
  className?: string
}) {
  return (
    <button
      type="button"
      aria-label={ariaLabel}
      title={title}
      onClick={onClick}
      className={className ?? 'rounded-lg bg-white/10 px-2 py-2 text-blue-100 transition hover:bg-white/20'}
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
  videoHardwareAccelerationOptions,
  audioEncodingOptions,
  onAddTrack,
  onUpdateTrack,
  onRemoveTrack
}: {
  tracks: EditableCallCatalogTrack[]
  videoEncodingOptions: VideoEncodingOptionSet
  videoHardwareAccelerationOptions: VideoHardwareAccelerationOption[]
  audioEncodingOptions: AudioEncodingOptionSet
  onAddTrack: (track: Omit<EditableCallCatalogTrack, 'id'>) => void
  onUpdateTrack: (id: string, patch: Partial<EditableCallCatalogTrack>) => void
  onRemoveTrack: (id: string) => void
}) {
  const videoTracks = tracks.filter((track) => track.role === 'video' && !isScreenShareTrackName(track.name))
  const screenshareTracks = tracks.filter((track) => track.role === 'video' && isScreenShareTrackName(track.name))
  const audioTracks = tracks.filter((track) => track.role === 'audio')
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
  const currentVideoResolutionOptions =
    currentDraft.role === 'video'
      ? getSupportedVideoResolutionOptions(videoEncodingOptions, currentDraft.codec)
      : videoEncodingOptions.resolutionOptions

  const setCurrentDraft = (patch: Partial<Omit<EditableCallCatalogTrack, 'id'>>) => {
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
        const validation = await validateVideoEncoderCatalogTrackConfig(currentDraft)
        if (!validation.ok) {
          setDraftConfigError(validation.message)
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
                    value={findVideoResolutionOptionIdFromList(
                      currentVideoResolutionOptions,
                      currentDraft.width,
                      currentDraft.height
                    )}
                    onChange={(event) => {
                      const option = currentVideoResolutionOptions.find((entry) => entry.id === event.target.value)
                      if (option) {
                        setCurrentDraft({ width: option.width, height: option.height })
                      }
                    }}
                  >
                    {currentVideoResolutionOptions.map((option) => (
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
                <label className="flex flex-col gap-1 text-xs text-blue-100">
                  <span>Hardware Acceleration</span>
                  <select
                    className="rounded border border-white/10 bg-white/10 px-2 py-1 text-sm text-white"
                    value={currentDraft.hardwareAcceleration ?? 'prefer-software'}
                    onChange={(event) =>
                      setCurrentDraft({
                        hardwareAcceleration: event.target.value as HardwareAcceleration
                      })
                    }
                  >
                    {videoHardwareAccelerationOptions.map((option) => (
                      <option key={option.id} value={option.value} className="bg-slate-900 text-white">
                        {option.label}
                      </option>
                    ))}
                  </select>
                </label>
                <label className="col-span-full flex max-w-xs flex-col gap-1 text-xs text-blue-100">
                  <span>Keyframe Interval (frames)</span>
                  <input
                    type="number"
                    min={1}
                    step={1}
                    value={normalizeTrackKeyframeInterval(currentDraft)}
                    onChange={(event) =>
                      setCurrentDraft({
                        keyframeInterval: toPositiveInteger(event.target.value, DEFAULT_VIDEO_KEYFRAME_INTERVAL)
                      })
                    }
                    className="rounded border border-white/10 bg-white/10 px-2 py-1 text-sm text-white"
                  />
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
                <div className="col-span-full rounded border border-white/10 bg-white/5 p-2">
                  <div className="text-xs font-semibold text-white">Audio Group Update</div>
                  <div className="mt-2 flex flex-wrap items-center gap-4">
                    <label className="inline-flex items-center gap-2 text-xs text-blue-100">
                      <input
                        type="radio"
                        name="draft-audio-stream-update-mode"
                        checked={resolveAudioStreamUpdateMode(currentDraft) === 'single'}
                        onChange={() => setCurrentDraft({ audioStreamUpdateMode: 'single' })}
                      />
                      <span>単一 Stream</span>
                    </label>
                    <label className="inline-flex items-center gap-2 text-xs text-blue-100">
                      <input
                        type="radio"
                        name="draft-audio-stream-update-mode"
                        checked={resolveAudioStreamUpdateMode(currentDraft) === 'interval'}
                        onChange={() => setCurrentDraft({ audioStreamUpdateMode: 'interval' })}
                      />
                      <span>N 秒ごとに更新</span>
                    </label>
                  </div>
                  {resolveAudioStreamUpdateMode(currentDraft) === 'interval' ? (
                    <label className="mt-2 flex max-w-xs flex-col gap-1 text-xs text-blue-100">
                      <span>更新間隔 (秒)</span>
                      <input
                        type="number"
                        min={1}
                        step={1}
                        value={resolveAudioStreamUpdateIntervalSeconds(currentDraft)}
                        onChange={(event) =>
                          setCurrentDraft({
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
                        <label className="flex max-w-xs flex-col gap-1 text-xs text-blue-100">
                          <span>Keyframe Interval (frames)</span>
                          <input
                            type="number"
                            min={1}
                            step={1}
                            value={normalizeTrackKeyframeInterval(track)}
                            onChange={(event) =>
                              onUpdateTrack(track.id, {
                                keyframeInterval: toPositiveInteger(event.target.value, DEFAULT_VIDEO_KEYFRAME_INTERVAL)
                              })
                            }
                            className="rounded border border-white/10 bg-white/10 px-2 py-1 text-sm text-white"
                          />
                        </label>
                      ) : (
                        <div className="space-y-2">
                          <div className="text-xs font-semibold text-white">Audio Group Update</div>
                          <div className="flex flex-wrap items-center gap-4">
                            <label className="inline-flex items-center gap-2 text-xs text-blue-100">
                              <input
                                type="radio"
                                name={`audio-stream-update-mode-${track.id}`}
                                checked={resolveAudioStreamUpdateMode(track) === 'single'}
                                onChange={() => onUpdateTrack(track.id, { audioStreamUpdateMode: 'single' })}
                              />
                              <span>単一 Stream</span>
                            </label>
                            <label className="inline-flex items-center gap-2 text-xs text-blue-100">
                              <input
                                type="radio"
                                name={`audio-stream-update-mode-${track.id}`}
                                checked={resolveAudioStreamUpdateMode(track) === 'interval'}
                                onChange={() => onUpdateTrack(track.id, { audioStreamUpdateMode: 'interval' })}
                              />
                              <span>N 秒ごとに更新</span>
                            </label>
                          </div>
                          {resolveAudioStreamUpdateMode(track) === 'interval' ? (
                            <label className="flex max-w-xs flex-col gap-1 text-xs text-blue-100">
                              <span>更新間隔 (秒)</span>
                              <input
                                type="number"
                                min={1}
                                step={1}
                                value={resolveAudioStreamUpdateIntervalSeconds(track)}
                                onChange={(event) =>
                                  onUpdateTrack(track.id, {
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

function findVideoCodecOptionId(options: VideoEncodingOptionSet, codec: string | undefined): string {
  return options.codecOptions.find((entry) => entry.codec === codec)?.id ?? options.codecOptions[0]?.id ?? ''
}

function createVideoTrackDraft(options: VideoEncodingOptionSet): Omit<EditableCallCatalogTrack, 'id'> {
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

function createScreenShareTrackDraft(options: VideoEncodingOptionSet): Omit<EditableCallCatalogTrack, 'id'> {
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
    framerate: undefined,
    hardwareAcceleration: undefined,
    keyframeInterval: undefined,
    samplerate: 48_000,
    channelConfig: channelConfigForChannels(channels),
    audioStreamUpdateMode: DEFAULT_AUDIO_STREAM_UPDATE_SETTINGS.mode,
    audioStreamUpdateIntervalSeconds: DEFAULT_AUDIO_STREAM_UPDATE_SETTINGS.intervalSeconds,
    isLive: true
  }
}

function findVideoResolutionOptionId(
  options: VideoEncodingOptionSet,
  width: number | undefined,
  height: number | undefined
): string {
  return findVideoResolutionOptionIdFromList(options.resolutionOptions, width, height)
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

function normalizeTrackKeyframeInterval(track: Pick<CallCatalogTrack, 'keyframeInterval'>): number {
  return toPositiveInteger(track.keyframeInterval, DEFAULT_VIDEO_KEYFRAME_INTERVAL)
}

function normalizeTrackFramerate(track: Pick<CallCatalogTrack, 'framerate'>): number {
  return toPositiveInteger(track.framerate, 30)
}

function resolveAudioStreamUpdateMode(track: Pick<CallCatalogTrack, 'audioStreamUpdateMode'>): 'single' | 'interval' {
  return track.audioStreamUpdateMode === 'single' ? 'single' : DEFAULT_AUDIO_STREAM_UPDATE_SETTINGS.mode
}

function resolveAudioStreamUpdateIntervalSeconds(
  track: Pick<CallCatalogTrack, 'audioStreamUpdateIntervalSeconds'>
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
  isChatSubscribed,
  isChatSubscribing,
  isChatUnsubscribing,
  onSelectTrack,
  onSubscribeVideo,
  onSubscribeScreenshare,
  onSubscribeAudio,
  onSubscribeChat,
  onUnsubscribeVideo,
  onUnsubscribeScreenshare,
  onUnsubscribeAudio,
  onUnsubscribeChat
}: {
  tracks: CallCatalogTrack[]
  selected?: { video?: string; screenshare?: string; audio?: string; chat?: string }
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
  isChatSubscribed: boolean
  isChatSubscribing: boolean
  isChatUnsubscribing: boolean
  onSelectTrack: (role: CatalogSubscribeRole, trackName: string) => void
  onSubscribeVideo: () => void
  onSubscribeScreenshare: () => void
  onSubscribeAudio: () => void
  onSubscribeChat: () => void
  onUnsubscribeVideo: () => void
  onUnsubscribeScreenshare: () => void
  onUnsubscribeAudio: () => void
  onUnsubscribeChat: () => void
}) {
  const videoTracks = tracks.filter((track) => track.role === 'video' && !isScreenShareTrackName(track.name))
  const screenshareTracks = tracks.filter((track) => track.role === 'video' && isScreenShareTrackName(track.name))
  const audioTracks = tracks.filter((track) => track.role === 'audio')
  const chatTracks = tracks.filter((track) => track.role === 'chat')
  const selectedVideo = selected?.video ?? videoTracks[0]?.name ?? ''
  const selectedScreenshare = selected?.screenshare ?? screenshareTracks[0]?.name ?? ''
  const selectedAudio = selected?.audio ?? audioTracks[0]?.name ?? ''
  const selectedChat = selected?.chat ?? chatTracks[0]?.name ?? ''
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
    },
    {
      role: 'chat',
      title: 'Chat Track',
      tracks: chatTracks,
      selectedTrack: selectedChat,
      subscribed: isChatSubscribed,
      subscribing: isChatSubscribing,
      unsubscribing: isChatUnsubscribing,
      onSubscribe: onSubscribeChat,
      onUnsubscribe: onUnsubscribeChat
    }
  ]
  const visibleTrackRows = trackRows.filter(
    (row) => row.tracks.length > 0 || row.subscribed || row.subscribing || row.unsubscribing
  )
  return (
    <div className="space-y-2 rounded-md border border-white/10 bg-white/5 p-3">
      <div className="flex items-center justify-between gap-2">
        <span className="text-sm font-semibold text-blue-100">Catalogs</span>
        <span className="text-[11px] text-blue-300">
          {isLoading ? 'Loading...' : hasCatalog ? 'Subscribed' : 'Waiting...'}
        </span>
      </div>
      {visibleTrackRows.map((row) => {
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
      <div className="grid grid-cols-[minmax(0,1fr)_auto_auto] items-center gap-2">
        <select
          value={selected}
          disabled={disabled}
          onChange={(event) => onChange(event.target.value)}
          className="w-full min-w-0 rounded border border-white/10 bg-white/10 px-2 py-1 text-sm text-white outline-none focus:border-blue-300 focus:ring-1 focus:ring-blue-300 disabled:opacity-60"
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
          aria-label={subscribeLabel}
          title={subscribeLabel}
          className={`inline-flex h-8 w-8 items-center justify-center rounded text-white transition ${
            canSubscribe ? 'bg-emerald-600 hover:bg-emerald-700' : 'bg-slate-500/70'
          } ${busy ? 'cursor-wait opacity-70' : ''} ${!canSubscribe ? 'cursor-not-allowed opacity-70' : ''}`}
        >
          <Plus className="h-4 w-4" />
          <span className="sr-only">{subscribeLabel}</span>
        </button>
        <button
          type="button"
          disabled={!canUnsubscribe}
          onClick={onUnsubscribe}
          aria-label={unsubscribeLabel}
          title={unsubscribeLabel}
          className={`inline-flex h-8 w-8 items-center justify-center rounded text-white transition ${
            canUnsubscribe ? 'bg-amber-600 hover:bg-amber-700' : 'bg-slate-500/70'
          } ${busy ? 'cursor-wait opacity-70' : ''} ${!canUnsubscribe ? 'cursor-not-allowed opacity-70' : ''}`}
        >
          <Minus className="h-4 w-4" />
          <span className="sr-only">{unsubscribeLabel}</span>
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

async function validateVideoEncoderCatalogTrackConfig(
  track: Pick<CallCatalogTrack, 'codec' | 'width' | 'height' | 'bitrate' | 'framerate' | 'hardwareAcceleration'>
): Promise<{ ok: true } | { ok: false; message: string }> {
  if (typeof VideoEncoder === 'undefined') {
    return { ok: false, message: 'VideoEncoder is not available in this browser.' }
  }
  if (!track.codec || !track.width || !track.height || !track.bitrate) {
    return { ok: false, message: 'Codec / resolution / bitrate must be set for video tracks.' }
  }
  const config: VideoEncoderConfig = {
    codec: track.codec,
    avc: track.codec.startsWith('avc') ? ({ format: 'annexb' } as VideoEncoderConfig['avc']) : undefined,
    width: Math.floor(track.width),
    height: Math.floor(track.height),
    bitrate: Math.floor(track.bitrate),
    framerate: Math.max(1, Math.min(120, Math.floor(track.framerate ?? 30))),
    hardwareAcceleration: track.hardwareAcceleration ?? 'prefer-software',
    scalabilityMode: 'L1T1',
    latencyMode: 'realtime'
  }
  try {
    const supported = await VideoEncoder.isConfigSupported(config)
    if (!supported.supported) {
      return { ok: false, message: `VideoEncoder config unsupported: ${track.codec}` }
    }
  } catch (error) {
    console.error('[call][catalog] VideoEncoder.isConfigSupported failed', error, config)
    return { ok: false, message: 'Failed to check VideoEncoder config support.' }
  }
  let encoder: VideoEncoder | undefined
  try {
    encoder = new VideoEncoder({
      output: (chunk) => {
        // No actual frames are encoded during configure check.
        void chunk
      },
      error: (error) => {
        console.error('[call][catalog] VideoEncoder error during configure check', error, config)
      }
    })
    encoder.configure(config)
    return { ok: true }
  } catch (error) {
    console.error('[call][catalog] VideoEncoder.configure failed', error, config)
    return { ok: false, message: `VideoEncoder.configure failed: ${track.codec}` }
  } finally {
    try {
      encoder?.close()
    } catch {
      // ignore
    }
  }
}

function CatalogsPanel({
  tracks,
  subscribedTracks
}: {
  tracks: EditableCallCatalogTrack[]
  subscribedTracks: SubscribedCatalogTrack[]
}) {
  const subscribedByName = new Map(subscribedTracks.map((track) => [track.name, track]))

  const sortTracks = (left: CallCatalogTrack, right: CallCatalogTrack) => {
    if (left.role !== right.role) {
      return left.role.localeCompare(right.role)
    }
    return (left.label || left.name).localeCompare(right.label || right.name)
  }

  const activeTracks = [...subscribedTracks].sort(sortTracks)

  return (
    <div className="space-y-3 rounded-md border border-white/10 bg-white/5 p-3">
      <div className="text-sm font-semibold text-blue-100">Catalogs</div>
      <TrackSettingsTiles
        title="Subscribed"
        tracks={activeTracks}
        getSubscriberCount={(trackName) => subscribedByName.get(trackName)?.subscriberCount ?? 0}
        emptyText="No active subscribers"
      />
    </div>
  )
}

function TrackSettingsTiles({
  title,
  tracks,
  getSubscriberCount,
  emptyText
}: {
  title: string
  tracks: CallCatalogTrack[]
  getSubscriberCount: (trackName: string) => number
  emptyText: string
}) {
  return (
    <div className="space-y-2 text-[10px] text-blue-100/90">
      <div className="text-[10px] font-medium text-blue-200/90">{title}</div>
      {tracks.length ? (
        <div className="grid grid-cols-1 gap-2 lg:grid-cols-2">
          {tracks.map((track) => (
            <TrackSettingTile
              key={track.name}
              title={`${track.label || track.name} (${formatTrackRoleLabel(track.role)})`}
              rows={[
                ['Track', track.name],
                ['Subscribers', `${getSubscriberCount(track.name)}`],
                ...buildCatalogTrackMetadataEntries(track).map<[string, string]>((entry) => [entry.label, entry.value])
              ]}
            />
          ))}
        </div>
      ) : (
        <div className="rounded border border-white/10 bg-white/[0.03] px-2 py-1 text-[10px] text-blue-200">
          {emptyText}
        </div>
      )}
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

function formatTrackRoleLabel(role: CallCatalogTrack['role']): string {
  if (role === 'video') {
    return 'Video'
  }
  if (role === 'audio') {
    return 'Audio'
  }
  return 'Chat'
}
