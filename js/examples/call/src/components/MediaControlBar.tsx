import { DeviceSelector } from './DeviceSelector'

interface MediaControlBarProps {
  videoDevices: MediaDeviceInfo[]
  audioDevices: MediaDeviceInfo[]
  selectedVideoDeviceId: string | null
  selectedAudioDeviceId: string | null
  onSelectVideoDevice: (deviceId: string) => void
  onSelectAudioDevice: (deviceId: string) => void
}

export function MediaControlBar({
  videoDevices,
  audioDevices,
  selectedVideoDeviceId,
  selectedAudioDeviceId,
  onSelectVideoDevice,
  onSelectAudioDevice
}: MediaControlBarProps) {
  return (
    <div className="mt-6 flex flex-wrap items-center gap-4 rounded-xl bg-white/10 px-4 py-3 shadow-lg backdrop-blur">
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
    </div>
  )
}
