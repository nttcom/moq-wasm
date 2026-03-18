interface DeviceSelectorProps {
  label: string
  devices: MediaDeviceInfo[]
  selectedDeviceId: string | null
  onChange: (deviceId: string) => void
}

export function DeviceSelector({ label, devices, selectedDeviceId, onChange }: DeviceSelectorProps) {
  return (
    <label className="flex items-center gap-2 text-sm text-blue-100">
      <span className="whitespace-nowrap font-semibold">{label}</span>
      <select
        className="min-w-[160px] rounded-md border border-white/10 bg-white/10 px-2 py-1 text-sm text-white outline-none focus:border-blue-300 focus:ring-1 focus:ring-blue-300"
        value={selectedDeviceId ?? ''}
        onChange={(e) => onChange(e.target.value)}
      >
        {devices.map((device) => (
          <option key={device.deviceId} value={device.deviceId} className="bg-slate-900 text-white">
            {device.label || `${label} ${device.deviceId.slice(0, 6)}`}
          </option>
        ))}
      </select>
    </label>
  )
}
