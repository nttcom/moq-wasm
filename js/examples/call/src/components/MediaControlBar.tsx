import { Mic, MicOff, Video, VideoOff } from 'lucide-react'

interface MediaControlBarProps {
  cameraEnabled: boolean
  microphoneEnabled: boolean
  cameraBusy: boolean
  microphoneBusy: boolean
  onToggleCamera: () => void
  onToggleMicrophone: () => void
}

export function MediaControlBar({
  cameraEnabled,
  microphoneEnabled,
  cameraBusy,
  microphoneBusy,
  onToggleCamera,
  onToggleMicrophone
}: MediaControlBarProps) {
  return (
    <div className="mt-6 flex items-center gap-4 rounded-xl bg-white/10 px-4 py-3 shadow-lg backdrop-blur">
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
        active={microphoneEnabled}
        disabled={microphoneBusy}
        onClick={onToggleMicrophone}
        activeLabel="Mic On"
        inactiveLabel="Mic Off"
        IconOn={Mic}
        IconOff={MicOff}
      />
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
      className={`flex items-center gap-2 rounded-lg px-4 py-2 text-sm font-semibold transition ${
        active ? 'bg-blue-500 text-white hover:bg-blue-600' : 'bg-white/10 text-blue-100 hover:bg-white/20'
      } ${disabled ? 'cursor-not-allowed opacity-60' : ''}`}
    >
      <Icon className="h-4 w-4" />
      <span>{active ? activeLabel : inactiveLabel}</span>
    </button>
  )
}
