import type { CaptureSettingsState } from '../types/captureConstraints'

interface GetUserMediaFormProps {
  settings: CaptureSettingsState
  onChange: (settings: Partial<CaptureSettingsState>) => void
  onApply: () => void
  cameraBusy: boolean
  microphoneBusy: boolean
  resolutionOptions: { id: string; label: string; width: number; height: number }[]
}

export function GetUserMediaForm({
  settings,
  onChange,
  onApply,
  cameraBusy,
  microphoneBusy,
  resolutionOptions
}: GetUserMediaFormProps) {
  const disabled = cameraBusy || microphoneBusy
  const matchedPreset = resolutionOptions.find((opt) => opt.width === settings.width && opt.height === settings.height)

  return (
    <div className="space-y-3 rounded-md border border-white/10 bg-white/5 p-3">
      <div className="flex items-center justify-between">
        <div className="text-sm font-semibold text-white">getUserMedia settings</div>
        <button
          type="button"
          onClick={onApply}
          disabled={disabled}
          className={`rounded-md px-3 py-1 text-sm font-semibold transition ${
            disabled ? 'bg-white/10 text-white/60' : 'bg-blue-500 text-white hover:bg-blue-600'
          }`}
        >
          Apply & getUserMedia
        </button>
      </div>
      <p className="text-xs text-blue-100/80">
        Configure capture before starting. Resolution/codec follow the selectors above.
      </p>
      <div className="grid grid-cols-2 gap-3 text-sm text-blue-50">
        <label className="flex items-center gap-2">
          <input
            type="checkbox"
            checked={settings.videoEnabled}
            onChange={(e) => onChange({ videoEnabled: e.target.checked })}
          />
          Enable camera
        </label>
        <label className="flex items-center gap-2">
          <input
            type="checkbox"
            checked={settings.audioEnabled}
            onChange={(e) => onChange({ audioEnabled: e.target.checked })}
          />
          Enable microphone
        </label>
        <label className="col-span-2 flex items-center justify-between gap-2">
          <span>Resolution preset</span>
          <select
            value={matchedPreset ? matchedPreset.id : 'custom'}
            onChange={(e) => {
              const next = resolutionOptions.find((opt) => opt.id === e.target.value)
              if (next) {
                onChange({ width: next.width, height: next.height })
              }
            }}
            className="w-40 rounded-md bg-white/10 px-2 py-1 text-sm text-white outline-none ring-1 ring-white/10 focus:ring-blue-400"
          >
            {resolutionOptions.map((opt) => (
              <option key={opt.id} value={opt.id}>
                {opt.label}
              </option>
            ))}
            {!matchedPreset && <option value="custom">{`${settings.width}x${settings.height} (custom)`}</option>}
          </select>
        </label>
        <div className="col-span-2 grid grid-cols-2 gap-3">
          <label className="flex items-center justify-between gap-2">
            <span>Width</span>
            <input
              type="number"
              min={160}
              max={7680}
              value={settings.width}
              onChange={(e) => onChange({ width: Number(e.target.value) || settings.width })}
              className="w-24 rounded-md bg-white/10 px-2 py-1 text-right text-white outline-none ring-1 ring-white/10 focus:ring-blue-400"
            />
          </label>
          <label className="flex items-center justify-between gap-2">
            <span>Height</span>
            <input
              type="number"
              min={120}
              max={4320}
              value={settings.height}
              onChange={(e) => onChange({ height: Number(e.target.value) || settings.height })}
              className="w-24 rounded-md bg-white/10 px-2 py-1 text-right text-white outline-none ring-1 ring-white/10 focus:ring-blue-400"
            />
          </label>
        </div>
        <label className="col-span-2 flex items-center justify-between gap-2">
          <span>Frame rate</span>
          <input
            type="number"
            min={1}
            max={120}
            value={settings.frameRate}
            onChange={(e) => onChange({ frameRate: Number(e.target.value) || settings.frameRate })}
            className="w-24 rounded-md bg-white/10 px-2 py-1 text-right text-white outline-none ring-1 ring-white/10 focus:ring-blue-400"
          />
        </label>
      </div>
      <div className="grid grid-cols-1 gap-2 text-sm text-blue-50">
        <label className="flex items-center justify-between gap-2">
          <span>Echo cancellation</span>
          <input
            type="checkbox"
            checked={settings.echoCancellation}
            onChange={(e) => onChange({ echoCancellation: e.target.checked })}
          />
        </label>
        <label className="flex items-center justify-between gap-2">
          <span>Noise suppression</span>
          <input
            type="checkbox"
            checked={settings.noiseSuppression}
            onChange={(e) => onChange({ noiseSuppression: e.target.checked })}
          />
        </label>
        <label className="flex items-center justify-between gap-2">
          <span>Auto gain control</span>
          <input
            type="checkbox"
            checked={settings.autoGainControl}
            onChange={(e) => onChange({ autoGainControl: e.target.checked })}
          />
        </label>
      </div>
    </div>
  )
}
