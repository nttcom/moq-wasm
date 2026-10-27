import { type ReactNode, useEffect, useRef, useState } from 'react'
import {
  BarChart3,
  Check,
  ChevronUp,
  CircleStop,
  LayoutGrid,
  MessageSquare,
  Mic,
  MicOff,
  Monitor,
  PhoneOff,
  Settings2,
  Shapes,
  SlidersHorizontal,
  User,
  Video,
  VideoOff
} from 'lucide-react'
import { ASR_CHOICES, type CaptionsControls } from '../hooks/useCaptions'
import type { RoomRecordingControls } from '../hooks/useRoomRecording'
import type { RecordingLayout } from '../recording/recorderApi'
import type { StageView } from '../utils/stageLayout'

interface ControlBarProps {
  cameraEnabled: boolean
  microphoneEnabled: boolean
  screenShareEnabled: boolean
  cameraBusy: boolean
  microphoneBusy: boolean
  onToggleCamera: () => void
  onToggleMicrophone: () => void
  onToggleScreenShare: () => void
  videoDevices: MediaDeviceInfo[]
  audioDevices: MediaDeviceInfo[]
  selectedVideoDeviceId: string | null
  selectedAudioDeviceId: string | null
  onSelectVideoDevice: (deviceId: string) => void
  onSelectAudioDevice: (deviceId: string) => void
  onOpenDevices: () => void
  onOpenCatalog: () => void
  onOpenStats: () => void
  view: StageView
  onToggleView: () => void
  chatOpen: boolean
  unreadChatMessageCount: number
  onToggleChat: () => void
  onLeave: () => void
  recording: RoomRecordingControls | null
  captions: CaptionsControls | null
}

export function ControlBar(props: ControlBarProps) {
  return (
    <div className="flex items-center justify-between gap-3 border-t border-white/10 bg-slate-950/90 px-4 py-3">
      <div className="flex items-center gap-2">
        <BarButton label="Catalog" onClick={props.onOpenCatalog}>
          <SlidersHorizontal className="h-5 w-5" />
        </BarButton>
        <BarButton label="Show stats" onClick={props.onOpenStats}>
          <BarChart3 className="h-5 w-5" />
        </BarButton>
      </div>
      <div className="flex items-center gap-3">
        <DeviceToggle
          label="Microphone"
          testId="microphone-options-button"
          devices={props.audioDevices}
          selectedDeviceId={props.selectedAudioDeviceId}
          onSelectDevice={props.onSelectAudioDevice}
          onOpenSettings={props.onOpenDevices}
        >
          <BarButton
            label={props.microphoneEnabled ? 'Turn microphone off' : 'Turn microphone on'}
            testId="toggle-microphone-button"
            off={!props.microphoneEnabled}
            disabled={props.microphoneBusy}
            onClick={props.onToggleMicrophone}
          >
            {props.microphoneEnabled ? <Mic className="h-5 w-5" /> : <MicOff className="h-5 w-5" />}
          </BarButton>
        </DeviceToggle>
        <DeviceToggle
          label="Camera"
          testId="camera-options-button"
          devices={props.videoDevices}
          selectedDeviceId={props.selectedVideoDeviceId}
          onSelectDevice={props.onSelectVideoDevice}
          onOpenSettings={props.onOpenDevices}
        >
          <BarButton
            label={props.cameraEnabled ? 'Turn camera off' : 'Turn camera on'}
            testId="toggle-camera-button"
            off={!props.cameraEnabled}
            disabled={props.cameraBusy}
            onClick={props.onToggleCamera}
          >
            {props.cameraEnabled ? <Video className="h-5 w-5" /> : <VideoOff className="h-5 w-5" />}
          </BarButton>
        </DeviceToggle>
        <BarButton
          label={props.screenShareEnabled ? 'Stop screen share' : 'Start screen share'}
          active={props.screenShareEnabled}
          disabled={props.cameraBusy}
          onClick={props.onToggleScreenShare}
        >
          <Monitor className="h-5 w-5" />
        </BarButton>
        <button
          type="button"
          data-testid="room-leave-button"
          onClick={props.onLeave}
          className="inline-flex h-12 items-center gap-2 rounded-full bg-red-600 px-5 text-sm font-semibold text-white transition hover:bg-red-700"
        >
          <PhoneOff className="h-5 w-5" />
          Leave
        </button>
      </div>
      <div className="flex items-center gap-2">
        {(props.recording || props.captions) && (
          <ActivitiesButton recording={props.recording} captions={props.captions} />
        )}
        <BarButton
          label={props.view === 'gallery' ? 'Speaker view' : 'Gallery view'}
          testId="stage-view-button"
          onClick={props.onToggleView}
        >
          {props.view === 'gallery' ? <User className="h-5 w-5" /> : <LayoutGrid className="h-5 w-5" />}
        </BarButton>
        <BarButton
          label={props.chatOpen ? 'Close chat' : 'Open chat'}
          testId="chat-toggle-button"
          active={props.chatOpen}
          onClick={props.onToggleChat}
        >
          <MessageSquare className="h-5 w-5" />
          {props.unreadChatMessageCount > 0 && (
            <span className="absolute -right-1 -top-1 flex h-5 min-w-5 items-center justify-center rounded-full bg-red-600 px-1 text-xs font-semibold">
              {props.unreadChatMessageCount}
            </span>
          )}
        </BarButton>
      </div>
    </div>
  )
}

/// Adds a chevron-opened device menu to the on/off button passed as `children`.
function DeviceToggle({
  label,
  testId,
  devices,
  selectedDeviceId,
  onSelectDevice,
  onOpenSettings,
  children
}: {
  label: string
  testId: string
  devices: MediaDeviceInfo[]
  selectedDeviceId: string | null
  onSelectDevice: (deviceId: string) => void
  onOpenSettings: () => void
  children: ReactNode
}) {
  const menu = useMenu()
  return (
    <div ref={menu.ref} className="relative flex items-center rounded-full bg-slate-800">
      <button
        type="button"
        aria-label={`${label} options`}
        title={`${label} options`}
        data-testid={testId}
        onClick={menu.toggle}
        className="inline-flex h-12 w-8 items-center justify-center rounded-l-full pl-1 text-slate-300 transition hover:text-white"
      >
        <ChevronUp className="h-4 w-4" />
      </button>
      {children}
      {menu.open && (
        <MenuPanel align="left">
          <MenuHeading>{label}</MenuHeading>
          {devices.length === 0 ? (
            <MenuNote>No {label.toLowerCase()} found</MenuNote>
          ) : (
            devices.map((device) => (
              <MenuItem
                key={device.deviceId}
                checked={device.deviceId === selectedDeviceId}
                onClick={() => {
                  menu.close()
                  onSelectDevice(device.deviceId)
                }}
              >
                {device.label || `${label} ${device.deviceId.slice(0, 6)}`}
              </MenuItem>
            ))
          )}
          <MenuDivider />
          <MenuItem
            onClick={() => {
              menu.close()
              onOpenSettings()
            }}
          >
            <Settings2 className="h-4 w-4" />
            Settings
          </MenuItem>
        </MenuPanel>
      )}
    </div>
  )
}

const RECORDING_CHOICES: { id: string; layout: RecordingLayout; composite: boolean; label: string }[] = [
  { id: 'composite', layout: 'combined', composite: true, label: 'All Track in one file' },
  { id: 'combined', layout: 'combined', composite: false, label: 'User Track in one file' },
  { id: 'per-track', layout: 'per-track', composite: false, label: 'Per Track in one file' }
]

function ActivitiesButton({
  recording,
  captions
}: {
  recording: RoomRecordingControls | null
  captions: CaptionsControls | null
}) {
  const menu = useMenu()
  const recordingLive = recording?.active !== undefined
  return (
    <div ref={menu.ref} className="relative">
      <BarButton label="Activities" testId="activities-button" active={menu.open} onClick={menu.toggle}>
        <Shapes className="h-5 w-5" />
        {recordingLive && <span className="absolute right-0 top-0 h-3 w-3 rounded-full bg-red-600" />}
      </BarButton>
      {menu.open && (
        <MenuPanel align="right">
          {recording && <RecordingSection recording={recording} onChoose={menu.close} />}
          {recording && captions && <MenuDivider />}
          {captions && <TranscriptionSection captions={captions} onChoose={menu.close} />}
        </MenuPanel>
      )}
    </div>
  )
}

function RecordingSection({ recording, onChoose }: { recording: RoomRecordingControls; onChoose: () => void }) {
  const active = recording.active
  const pending =
    active?.state === 'starting'
      ? 'Starting the recording'
      : active?.state === 'stopping'
        ? 'Stopping the recording'
        : null
  return (
    <>
      <MenuHeading>Recording</MenuHeading>
      {active ? (
        <MenuItem
          testId="record-stop"
          disabled={pending !== null || recording.busy}
          onClick={() => {
            onChoose()
            void recording.stop()
          }}
        >
          <CircleStop className="h-4 w-4 text-red-400" />
          {pending ?? 'Stop recording'}
        </MenuItem>
      ) : (
        RECORDING_CHOICES.map(({ id, layout, composite, label }) => (
          <MenuItem
            key={id}
            testId={`record-${id}`}
            disabled={recording.busy}
            onClick={() => {
              onChoose()
              void recording.start(layout, composite)
            }}
          >
            {label}
          </MenuItem>
        ))
      )}
      {recording.error && <MenuNote>{recording.error}</MenuNote>}
    </>
  )
}

function TranscriptionSection({ captions, onChoose }: { captions: CaptionsControls; onChoose: () => void }) {
  const items: { id: string; label: string; checked?: boolean; onClick: () => void }[] = captions.active
    ? [
        { id: 'show', label: 'Show captions', checked: captions.on, onClick: () => captions.show(true) },
        { id: 'hide', label: 'Hide captions', checked: !captions.on, onClick: () => captions.show(false) },
        {
          id: 'stop',
          label: 'Stop transcription',
          onClick: () => {
            if (window.confirm('Stopping transcription stops it for everyone in the room.')) {
              captions.stop()
            }
          }
        }
      ]
    : ASR_CHOICES.map(({ asr, label }) => ({ id: asr, label, onClick: () => captions.start(asr) }))
  return (
    <>
      <MenuHeading>Transcription</MenuHeading>
      {items.map(({ id, label, checked, onClick }) => (
        <MenuItem
          key={id}
          testId={`captions-${id}`}
          checked={checked}
          onClick={() => {
            onChoose()
            onClick()
          }}
        >
          {label}
        </MenuItem>
      ))}
      {!captions.active && captions.error && <MenuNote testId="captions-error">{captions.error}</MenuNote>}
    </>
  )
}

/// `ref` must wrap the trigger as well as the panel: a pointerdown outside it, or Escape, closes the menu.
function useMenu() {
  const [open, setOpen] = useState(false)
  const ref = useRef<HTMLDivElement>(null)
  useEffect(() => {
    if (!open) {
      return
    }
    const closeOutside = (event: PointerEvent) => {
      if (!ref.current?.contains(event.target as Node)) {
        setOpen(false)
      }
    }
    const closeOnEscape = (event: KeyboardEvent) => {
      if (event.key === 'Escape') {
        setOpen(false)
      }
    }
    document.addEventListener('pointerdown', closeOutside)
    document.addEventListener('keydown', closeOnEscape)
    return () => {
      document.removeEventListener('pointerdown', closeOutside)
      document.removeEventListener('keydown', closeOnEscape)
    }
  }, [open])
  return { open, ref, toggle: () => setOpen((current) => !current), close: () => setOpen(false) }
}

function MenuPanel({ align, children }: { align: 'left' | 'right'; children: ReactNode }) {
  return (
    <div
      role="menu"
      className={`absolute bottom-14 z-20 w-64 rounded-lg border border-white/10 bg-slate-900 p-1 shadow-lg ${
        align === 'left' ? 'left-0' : 'right-0'
      }`}
    >
      {children}
    </div>
  )
}

function MenuHeading({ children }: { children: ReactNode }) {
  return <div className="px-3 pb-1 pt-2 text-xs font-semibold uppercase tracking-wide text-slate-400">{children}</div>
}

function MenuNote({ testId, children }: { testId?: string; children: ReactNode }) {
  return (
    <p data-testid={testId} className="px-3 py-2 text-xs text-slate-400">
      {children}
    </p>
  )
}

function MenuDivider() {
  return <div className="my-1 border-t border-white/10" />
}

function MenuItem({
  testId,
  checked,
  disabled = false,
  onClick,
  children
}: {
  testId?: string
  /// `undefined` makes a plain item; `false` still reserves the check column.
  checked?: boolean
  disabled?: boolean
  onClick: () => void
  children: ReactNode
}) {
  return (
    <button
      type="button"
      role={checked === undefined ? 'menuitem' : 'menuitemradio'}
      data-testid={testId}
      aria-checked={checked}
      disabled={disabled}
      onClick={onClick}
      className="flex w-full items-center gap-2 rounded-md px-3 py-2 text-left text-sm hover:bg-slate-700 disabled:cursor-not-allowed disabled:opacity-60 disabled:hover:bg-transparent"
    >
      {checked !== undefined && <Check className={`h-4 w-4 shrink-0 ${checked ? '' : 'invisible'}`} />}
      <span className="flex min-w-0 items-center gap-2 truncate">{children}</span>
    </button>
  )
}

function BarButton({
  label,
  testId,
  off = false,
  active = false,
  disabled = false,
  onClick,
  children
}: {
  label: string
  testId?: string
  off?: boolean
  active?: boolean
  disabled?: boolean
  onClick: () => void
  children: ReactNode
}) {
  const tone = off
    ? 'bg-red-600 hover:bg-red-700'
    : active
      ? 'bg-blue-600 hover:bg-blue-700'
      : 'bg-slate-700 hover:bg-slate-600'
  return (
    <button
      type="button"
      aria-label={label}
      title={label}
      data-testid={testId}
      onClick={onClick}
      disabled={disabled}
      className={`relative inline-flex h-12 w-12 items-center justify-center rounded-full text-white transition ${tone} ${
        disabled ? 'cursor-not-allowed opacity-60' : ''
      }`}
    >
      {children}
    </button>
  )
}
