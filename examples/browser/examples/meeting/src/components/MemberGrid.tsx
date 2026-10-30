import { LocalMember, RemoteMember } from '../types/member'
import type { CaptionsControls } from '../hooks/useCaptions'
import type { RoomRecordingControls } from '../hooks/useRoomRecording'
import { ReactNode, useEffect, useState } from 'react'
import { PanelRightClose, PanelRightOpen } from 'lucide-react'
import type { PlayoutSettings } from '../types/playout'
import { PlayoutControls } from './PlayoutControls'
import { ControlBar } from './ControlBar'
import { ParticipantTile } from './ParticipantTile'
import { LOCAL_MEMBER_ID, type StageArrangement, type StageTile, galleryTileWidth } from '../utils/stageLayout'
import type { CaptureSettingsState } from '../types/captureConstraints'
import { GetUserMediaForm } from './GetUserMediaForm'
import {
  CATALOG_SUBSCRIBE_ROLES,
  type CatalogSelections,
  type CatalogSubscribeRole,
  type MeetingCatalogTrack,
  AUTO_TRACK_SELECTION,
  isSwitchableRole
} from '../types/catalog'
import { catalogTracksForRole } from '../media/meetingCatalog'
import { buildTrackActionKey } from '../utils/state/roomState'
import type { RemoteStatsSample } from '../types/stats'
import { MeetingStats, RemoteMemberStatsCharts } from './StatsCharts'
import type { RemotePictures } from '../media/mediaSubscriber'
import { formatLiveStats } from '@player/locLive'
import type {
  VideoHardwareAccelerationOption,
  VideoResolutionOption,
  VideoEncodingSettings
} from '../types/videoEncoding'

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
  videoResolutionOptions: VideoResolutionOption[]
  videoHardwareAccelerationOptions: VideoHardwareAccelerationOption[]
  selectedVideoEncoding: VideoEncodingSettings
  onSelectVideoEncoding: (settings: Partial<VideoEncodingSettings>) => Promise<void>
  selectedScreenShareEncoding: VideoEncodingSettings
  onSelectScreenShareEncoding: (settings: Partial<VideoEncodingSettings>) => Promise<void>
  captureSettings: CaptureSettingsState
  onChangeCaptureSettings: (settings: Partial<CaptureSettingsState>) => void
  onApplyCaptureSettings: () => void
  remoteCatalogTracks: Map<string, MeetingCatalogTrack[]>
  remoteCatalogSelections: Map<string, CatalogSelections>
  manualTrackKeys: Set<string>
  catalogLoadingMemberIds: Set<string>
  catalogSubscribedMemberIds: Set<string>
  catalogUnsubscribingTrackKeys: Set<string>
  onSelectCatalogTrack: (memberId: string, role: CatalogSubscribeRole, trackName: string) => void
  remoteStatsHistory: Map<string, RemoteStatsSample[]>
  stage: StageArrangement
  speakingMemberIds: Set<string>
  filmstripOpen: boolean
  onToggleView: () => void
  onToggleFilmstrip: () => void
  chatOpen: boolean
  unreadChatMessageCount: number
  recording: RoomRecordingControls | null
  captions: CaptionsControls | null
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
  videoResolutionOptions,
  videoHardwareAccelerationOptions,
  selectedVideoEncoding,
  onSelectVideoEncoding,
  selectedScreenShareEncoding,
  onSelectScreenShareEncoding,
  captureSettings,
  onChangeCaptureSettings,
  onApplyCaptureSettings,
  remoteCatalogTracks,
  remoteCatalogSelections,
  manualTrackKeys,
  catalogLoadingMemberIds,
  catalogSubscribedMemberIds,
  catalogUnsubscribingTrackKeys,
  onSelectCatalogTrack,
  remoteStatsHistory,
  stage,
  speakingMemberIds,
  filmstripOpen,
  onToggleView,
  onToggleFilmstrip,
  chatOpen,
  unreadChatMessageCount,
  recording,
  captions,
  onToggleChat,
  onLeave,
  onTileHeight
}: MemberGridProps) {
  const [isDeviceModalOpen, setIsDeviceModalOpen] = useState(false)
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
          caption={share ? undefined : captions?.captions.get(localMember.name)}
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
          heightKey={tile.key}
          onHeight={onTileHeight}
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
        caption={captions?.captions.get(member.name)}
        heightKey={tile.key}
        onHeight={onTileHeight}
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
            {stage.others.length > 0 &&
              (filmstripOpen ? (
                <div className="flex w-56 shrink-0 flex-col gap-3 overflow-y-auto" data-testid="filmstrip">
                  <div className="flex justify-end">
                    <FilmstripButton open onClick={onToggleFilmstrip} />
                  </div>
                  {stage.others.map((tile) => (
                    <div key={tile.key} className="aspect-video w-full shrink-0">
                      {renderTile(tile)}
                    </div>
                  ))}
                </div>
              ) : (
                <div className="shrink-0">
                  <FilmstripButton open={false} onClick={onToggleFilmstrip} />
                </div>
              ))}
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
        videoDevices={videoDevices}
        audioDevices={audioDevices}
        selectedVideoDeviceId={selectedVideoDeviceId}
        selectedAudioDeviceId={selectedAudioDeviceId}
        onSelectVideoDevice={onSelectVideoDevice}
        onSelectAudioDevice={onSelectAudioDevice}
        onOpenDevices={() => setIsDeviceModalOpen(true)}
        onOpenStats={() => setIsStatsModalOpen(true)}
        view={stage.view}
        onToggleView={onToggleView}
        chatOpen={chatOpen}
        unreadChatMessageCount={unreadChatMessageCount}
        onToggleChat={onToggleChat}
        onLeave={onLeave}
        recording={recording}
        captions={captions}
      />
      {isDeviceModalOpen && (
        <DeviceModal title="Device settings" onClose={() => setIsDeviceModalOpen(false)}>
          <div className="space-y-4">
            <GetUserMediaForm
              settings={captureSettings}
              onChange={onChangeCaptureSettings}
              onApply={onApplyCaptureSettings}
              cameraBusy={cameraBusy}
              microphoneBusy={microphoneBusy}
              resolutionOptions={videoResolutionOptions}
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
      {isStatsModalOpen && (
        <DeviceModal title="Stats" onClose={() => setIsStatsModalOpen(false)} size="wide">
          <MeetingStats
            remoteMembers={remoteMembers.map((member) => ({
              id: member.id,
              name: member.name,
              samples: remoteStatsHistory.get(member.id) ?? []
            }))}
          />
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

function FilmstripButton({ open, onClick }: { open: boolean; onClick: () => void }) {
  const label = open ? 'Hide participants' : 'Show participants'
  return (
    <button
      type="button"
      aria-label={label}
      title={label}
      data-testid="filmstrip-button"
      onClick={onClick}
      className="inline-flex h-8 w-8 items-center justify-center rounded-md bg-slate-800 text-slate-200 transition hover:bg-slate-700 hover:text-white"
    >
      {open ? <PanelRightClose className="h-4 w-4" /> : <PanelRightOpen className="h-4 w-4" />}
    </button>
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

function formatTrackBitrateForDisplay(bitrate: number): string {
  if (bitrate >= 1_000_000) {
    const mbps = bitrate / 1_000_000
    const value = Number.isInteger(mbps) ? mbps.toFixed(0) : mbps.toFixed(1)
    return `${value}Mbps`
  }
  return `${Math.round(bitrate / 1000)}kbps`
}
