import type { ReactNode } from 'react'
import {
  BarChart3,
  LayoutGrid,
  MessageSquare,
  Mic,
  MicOff,
  Monitor,
  PanelRightClose,
  PanelRightOpen,
  PhoneOff,
  Settings2,
  SlidersHorizontal,
  User,
  Video,
  VideoOff
} from 'lucide-react'
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
  onOpenDevices: () => void
  onOpenCatalog: () => void
  onOpenStats: () => void
  view: StageView
  onToggleView: () => void
  /// Undefined while the stage shows no filmstrip.
  filmstripOpen: boolean | undefined
  onToggleFilmstrip: () => void
  chatOpen: boolean
  onToggleChat: () => void
  onLeave: () => void
}

export function ControlBar(props: ControlBarProps) {
  return (
    <div className="flex items-center justify-between gap-3 border-t border-white/10 bg-slate-950/90 px-4 py-3">
      <div className="flex items-center gap-2">
        <BarButton label="Device settings" onClick={props.onOpenDevices}>
          <Settings2 className="h-5 w-5" />
        </BarButton>
        <BarButton label="Catalog" onClick={props.onOpenCatalog}>
          <SlidersHorizontal className="h-5 w-5" />
        </BarButton>
        <BarButton label="Show stats" onClick={props.onOpenStats}>
          <BarChart3 className="h-5 w-5" />
        </BarButton>
      </div>
      <div className="flex items-center gap-3">
        <BarButton
          label={props.microphoneEnabled ? 'Turn microphone off' : 'Turn microphone on'}
          testId="toggle-microphone-button"
          off={!props.microphoneEnabled}
          disabled={props.microphoneBusy}
          onClick={props.onToggleMicrophone}
        >
          {props.microphoneEnabled ? <Mic className="h-5 w-5" /> : <MicOff className="h-5 w-5" />}
        </BarButton>
        <BarButton
          label={props.cameraEnabled ? 'Turn camera off' : 'Turn camera on'}
          testId="toggle-camera-button"
          off={!props.cameraEnabled}
          disabled={props.cameraBusy}
          onClick={props.onToggleCamera}
        >
          {props.cameraEnabled ? <Video className="h-5 w-5" /> : <VideoOff className="h-5 w-5" />}
        </BarButton>
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
        <BarButton
          label={props.view === 'gallery' ? 'Speaker view' : 'Gallery view'}
          testId="stage-view-button"
          onClick={props.onToggleView}
        >
          {props.view === 'gallery' ? <User className="h-5 w-5" /> : <LayoutGrid className="h-5 w-5" />}
        </BarButton>
        {props.filmstripOpen !== undefined && (
          <BarButton
            label={props.filmstripOpen ? 'Hide participants' : 'Show participants'}
            testId="filmstrip-button"
            onClick={props.onToggleFilmstrip}
          >
            {props.filmstripOpen ? <PanelRightClose className="h-5 w-5" /> : <PanelRightOpen className="h-5 w-5" />}
          </BarButton>
        )}
        <BarButton
          label={props.chatOpen ? 'Close chat' : 'Open chat'}
          testId="chat-toggle-button"
          active={props.chatOpen}
          onClick={props.onToggleChat}
        >
          <MessageSquare className="h-5 w-5" />
        </BarButton>
      </div>
    </div>
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
      className={`inline-flex h-12 w-12 items-center justify-center rounded-full text-white transition ${tone} ${
        disabled ? 'cursor-not-allowed opacity-60' : ''
      }`}
    >
      {children}
    </button>
  )
}
