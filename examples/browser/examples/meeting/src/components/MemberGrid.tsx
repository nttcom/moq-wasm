import { LocalMember, RemoteMember } from '../types/member'
import type { CaptionsControls } from '../hooks/useCaptions'
import type { RoomRecordingControls } from '../hooks/useRoomRecording'
import { ReactNode, useEffect, useLayoutEffect, useRef, useState } from 'react'
import { PanelRightClose, PanelRightOpen, X } from 'lucide-react'
import type { PlayoutSettings } from '../types/playout'
import { PlayoutControls } from './PlayoutControls'
import { ControlBar } from './ControlBar'
import { ParticipantTile } from './ParticipantTile'
import { LOCAL_MEMBER_ID, type StageArrangement, type StageTile, galleryTileWidth } from '../utils/stageLayout'
import type { CaptureSettingsState } from '../types/captureConstraints'
import { GetUserMediaForm } from './GetUserMediaForm'
import { AUTO_TRACK_SELECTION, type CatalogSubscribeRole } from '../types/catalog'
import type { RemoteStatsSample } from '../types/stats'
import { MeetingStats, RemoteMemberStatsCharts } from './StatsCharts'
import type { RemoteMemberView, RemoteSourceView, RemoteVideoSource } from '../media/remotePlayers'
import { type MediaCatalogTrack, formatLiveStats } from 'moq-wasm/media'
import {
  VIDEO_HARDWARE_ACCELERATION_OPTIONS,
  VIDEO_RESOLUTION_OPTIONS,
  type VideoEncodingSettings
} from '../types/videoEncoding'
import { FIELD_CLASS } from '../../../../lib/ui/field'

const DEVICE_MODAL_FIELD_CLASS = 'flex items-center justify-between gap-3 text-sm text-fg'

interface MemberGridProps {
  localMember: LocalMember
  remoteMembers: RemoteMember[]
  localVideoStream?: MediaStream | null
  localScreenShareStream?: MediaStream | null
  remoteViews: Map<string, RemoteMemberView>
  /// The element a remote source's player draws into.
  remoteContainer: (memberId: string, source: RemoteVideoSource) => HTMLElement | undefined
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
  selectedVideoEncoding: VideoEncodingSettings
  onSelectVideoEncoding: (settings: Partial<VideoEncodingSettings>) => Promise<void>
  selectedScreenShareEncoding: VideoEncodingSettings
  onSelectScreenShareEncoding: (settings: Partial<VideoEncodingSettings>) => Promise<void>
  captureSettings: CaptureSettingsState
  onChangeCaptureSettings: (settings: Partial<CaptureSettingsState>) => void
  onApplyCaptureSettings: () => void
  onSelectTrack: (memberId: string, role: CatalogSubscribeRole, trackName: string) => void
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
}

export function MemberGrid({
  localMember,
  remoteMembers,
  localVideoStream,
  localScreenShareStream,
  remoteViews,
  remoteContainer,
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
  selectedVideoEncoding,
  onSelectVideoEncoding,
  selectedScreenShareEncoding,
  onSelectScreenShareEncoding,
  captureSettings,
  onChangeCaptureSettings,
  onApplyCaptureSettings,
  onSelectTrack,
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
  onLeave
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
    const view = remoteViews.get(member.id)
    const picture = {
      element: remoteContainer(member.id, tile.source),
      shown: view?.[tile.source].showsPicture ?? false
    }
    if (tile.source === 'screenshare') {
      return (
        <ParticipantTile
          name={member.name}
          testId={`member-share-${member.name}`}
          picture={picture}
          label="画面共有"
          speaking={false}
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
        picture={picture}
        speaking={speakingMemberIds.has(member.id)}
        caption={captions?.captions.get(member.name)}
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
        <DeviceModal title="デバイス設定" onClose={() => setIsDeviceModalOpen(false)}>
          <div className="space-y-4">
            <GetUserMediaForm
              settings={captureSettings}
              onChange={onChangeCaptureSettings}
              onApply={onApplyCaptureSettings}
              cameraBusy={cameraBusy}
              microphoneBusy={microphoneBusy}
              resolutionOptions={VIDEO_RESOLUTION_OPTIONS}
            />
            <div className="space-y-3 rounded-lg border border-line bg-subtle p-4">
              <div className="text-sm font-semibold text-fg">映像エンコーダーのハードウェアアクセラレーション</div>
              <OptionSelect
                label="カメラ"
                className={DEVICE_MODAL_FIELD_CLASS}
                options={VIDEO_HARDWARE_ACCELERATION_OPTIONS}
                value={selectedVideoEncoding.hardwareAcceleration}
                onChange={(option) => void onSelectVideoEncoding({ hardwareAcceleration: option.value })}
              />
              <OptionSelect
                label="画面共有"
                className={DEVICE_MODAL_FIELD_CLASS}
                options={VIDEO_HARDWARE_ACCELERATION_OPTIONS}
                value={selectedScreenShareEncoding.hardwareAcceleration}
                onChange={(option) => void onSelectScreenShareEncoding({ hardwareAcceleration: option.value })}
              />
            </div>
          </div>
        </DeviceModal>
      )}
      {isStatsModalOpen && (
        <DeviceModal title="統計" onClose={() => setIsStatsModalOpen(false)} size="wide">
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
                view={remoteViews.get(detailsMember.id)}
                onSelectTrack={(role, trackName) => onSelectTrack(detailsMember.id, role, trackName)}
              />
              <div className="space-y-3 rounded-lg border border-line bg-subtle p-4">
                <div className="text-sm font-semibold text-fg">Playout</div>
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
      className="inline-flex h-8 w-8 cursor-pointer items-center justify-center rounded-md border border-line bg-surface text-muted transition hover:bg-hover hover:text-fg"
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
export function memberStates(member: RemoteMember, view: RemoteMemberView | undefined): Record<string, string> {
  return {
    'data-catalog-status': formatCatalogStatus(view),
    ...Object.fromEntries(trackRows(member, view).map((row) => [`data-${row.role}-status`, formatSubscribeStatus(row)]))
  }
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
  const dialogRef = useRef<HTMLDialogElement>(null)
  // A modal <dialog> moves focus into itself, keeps it there, closes on Escape and gives focus back. The
  // layout effect's cleanup closes it while it is still in the document, which focus return needs.
  useLayoutEffect(() => {
    const dialog = dialogRef.current
    dialog?.showModal()
    return () => dialog?.close()
  }, [])
  return (
    <dialog
      ref={dialogRef}
      aria-label={title}
      onCancel={(event) => {
        event.preventDefault()
        onClose()
      }}
      // A click on the backdrop lands on the dialog itself; the content fills the rest of it.
      onClick={(event) => {
        if (event.target === event.currentTarget) onClose()
      }}
      className={`m-auto w-[calc(100%-2rem)] ${maxWidthClass} max-h-[90vh] overflow-y-auto rounded-xl border border-line-strong bg-surface p-0 text-fg shadow-2xl backdrop:bg-black/60 backdrop:backdrop-blur-sm`}
    >
      <div className="p-6">
        <div className="mb-5 flex items-center justify-between gap-4">
          <h4 className="text-lg font-semibold tracking-tight">{title}</h4>
          <button
            type="button"
            onClick={onClose}
            data-testid="modal-close-button"
            className="inline-flex h-8 cursor-pointer items-center gap-1.5 rounded-md border border-line-strong bg-raised px-3 text-sm font-medium text-fg transition hover:bg-hover"
          >
            <X className="h-4 w-4" />
            閉じる
          </button>
        </div>
        {children}
      </div>
    </dialog>
  )
}

function OptionSelect<T extends { id: string; label: string }>({
  label,
  options,
  value,
  onChange,
  className = 'flex flex-col gap-1 text-xs text-fg'
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
        className={FIELD_CLASS}
        value={value}
        onChange={(event) => {
          const option = options.find((entry) => entry.id === event.target.value)
          if (option) {
            onChange(option)
          }
        }}
      >
        {options.map((option) => (
          <option key={option.id} value={option.id} className="bg-surface text-fg">
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

type TrackRow = {
  role: CatalogSubscribeRole
  tracks: MediaCatalogTrack[]
  selected: string
  /// Undefined for a role without adaptation.
  auto?: boolean
  subscribed: boolean
  subscribing: boolean
}

function trackRows(member: RemoteMember, view: RemoteMemberView | undefined): TrackRow[] {
  const videoRow = (role: CatalogSubscribeRole, source: RemoteSourceView | undefined): TrackRow => ({
    role,
    tracks: source?.videoTracks ?? [],
    selected: source?.selectedVideoTrack ?? '',
    auto: source?.adaptiveVideo ?? true,
    subscribed: source?.videoSubscribed ?? false,
    subscribing: false
  })
  return [
    videoRow('video', view?.camera),
    videoRow('screenshare', view?.screenshare),
    {
      role: 'audio',
      tracks: view?.audioTracks ?? [],
      selected: view?.selectedAudioTrack ?? '',
      subscribed: view?.audioSubscribed ?? false,
      subscribing: false
    },
    {
      role: 'chat',
      tracks: [{ name: 'chat', label: 'chat' }],
      selected: 'chat',
      subscribed: member.chat.isSubscribed,
      subscribing: member.chat.isSubscribing
    }
  ]
}

function RemoteCatalogSubscribePanel({
  member,
  view,
  onSelectTrack
}: {
  member: RemoteMember
  view: RemoteMemberView | undefined
  onSelectTrack: (role: CatalogSubscribeRole, trackName: string) => void
}) {
  const memberName = member.name
  const catalogLoaded = view?.catalog === 'loaded'
  const visibleTrackRows = trackRows(member, view).filter(
    (row) => row.role === 'chat' || row.tracks.length > 0 || row.subscribed
  )
  return (
    <div className="space-y-2 rounded-lg border border-line bg-subtle p-4">
      <div className="flex items-center justify-between gap-2">
        <span className="text-sm font-semibold text-fg">Catalogs</span>
        <span data-testid={`catalog-status-${memberName}`} className="text-[11px] text-accent">
          {formatCatalogStatus(view)}
        </span>
      </div>
      {visibleTrackRows.map((row) => (
        <TrackSubscribeRow
          key={row.role}
          title={SUBSCRIBE_ROW_TITLES[row.role]}
          role={row.role}
          tracks={row.tracks}
          selected={row.selected}
          auto={row.auto}
          disabled={!catalogLoaded || row.role === 'chat'}
          statusText={formatSubscribeStatus(row)}
          statusTestId={`track-status-${row.role}-${memberName}`}
          selectTestId={`track-select-${row.role}-${memberName}`}
          onChange={(trackName) => onSelectTrack(row.role, trackName)}
        />
      ))}
    </div>
  )
}

/// `auto` is undefined for a role without adaptation; otherwise the select
/// offers Auto and `auto` tells whether it is selected.
function TrackSubscribeRow({
  title,
  role,
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
  role: CatalogSubscribeRole
  tracks: MediaCatalogTrack[]
  selected: string
  auto?: boolean
  disabled: boolean
  statusText: string
  statusTestId?: string
  selectTestId?: string
  onChange: (trackName: string) => void
}) {
  return (
    <div className="space-y-1 rounded-md border border-line bg-surface p-2.5">
      <div className="flex items-center justify-between gap-2">
        <span className="text-xs font-semibold text-fg">{title}</span>
        <span data-testid={statusTestId} className="text-[11px] text-accent">
          {statusText}
        </span>
      </div>
      <div>
        <select
          value={auto ? AUTO_TRACK_SELECTION : selected}
          disabled={disabled}
          data-testid={selectTestId}
          onChange={(event) => onChange(event.target.value)}
          className={`w-full min-w-0 ${FIELD_CLASS} disabled:opacity-60`}
        >
          {tracks.length === 0 ? (
            <option value="" className="bg-surface text-fg">
              No tracks
            </option>
          ) : (
            <>
              {auto !== undefined && (
                <option value={AUTO_TRACK_SELECTION} className="bg-surface text-fg">
                  {`Auto (${tracks.find((track) => track.name === selected)?.label ?? selected})`}
                </option>
              )}
              {tracks.map((track) => (
                <option key={track.name} value={track.name} className="bg-surface text-fg">
                  {formatCatalogTrackLabel(role, track)}
                </option>
              ))}
            </>
          )}
        </select>
      </div>
    </div>
  )
}

function formatCatalogStatus(view: RemoteMemberView | undefined): string {
  if (view?.catalog === 'loaded') {
    return 'Subscribed'
  }
  return view?.catalog === 'absent' ? 'Waiting...' : 'Loading...'
}

function formatSubscribeStatus({ subscribed, subscribing }: TrackRow): string {
  if (subscribing) {
    return 'Subscribing...'
  }
  return subscribed ? 'Subscribed' : 'Not subscribed'
}

function formatCatalogTrackLabel(role: CatalogSubscribeRole, track: MediaCatalogTrack): string {
  const parts: string[] = [track.label || track.name]
  if (typeof track.bitrate === 'number') {
    parts.push(formatTrackBitrateForDisplay(track.bitrate))
  }
  if (role !== 'audio' && typeof track.width === 'number' && typeof track.height === 'number') {
    parts.push(`${track.width}x${track.height}`)
  }
  if (role === 'audio' && typeof track.samplerate === 'number') {
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
