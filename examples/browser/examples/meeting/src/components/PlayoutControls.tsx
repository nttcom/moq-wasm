import type { CatchUp } from '@player/livePlayout'
import { DEFAULT_PLAYOUT_SETTINGS, type PlayoutSettings } from '../types/playout'

const CATCH_UP_OPTIONS: { value: CatchUp; label: string }[] = [
  { value: 'skip', label: 'Skip (trim + crossfade)' },
  { value: 'speed-up', label: 'Speed up (WSOLA)' },
  { value: 'off', label: 'Off' }
]

const CONTROL_CLASS =
  'w-40 rounded-md bg-white/10 px-2 py-1 text-sm text-white outline-none ring-1 ring-white/10 focus:ring-blue-400'

interface PlayoutControlsProps {
  value?: PlayoutSettings
  onChange: (value: PlayoutSettings) => void
}

export function PlayoutControls({ value, onChange }: PlayoutControlsProps) {
  const settings = value ?? DEFAULT_PLAYOUT_SETTINGS
  const { minimumMs, maximumMs } = settings.policy

  const changePolicy = (patch: Partial<PlayoutSettings['policy']>) =>
    onChange({ ...settings, policy: { ...settings.policy, ...patch } })

  return (
    <div className="space-y-3 rounded-lg border border-white/10 bg-white/5 p-3">
      <div className="text-xs font-semibold uppercase tracking-wide text-blue-100">Playout Buffer</div>
      <label className="flex items-center justify-between gap-3 text-sm text-blue-50">
        <span>Min buffer (ms)</span>
        <input
          type="number"
          min={0}
          max={5000}
          step={50}
          value={minimumMs}
          onChange={(event) => changePolicy({ minimumMs: nonNegativeNumber(event.target.value, minimumMs) })}
          className={CONTROL_CLASS}
        />
      </label>
      <label className="flex items-center justify-between gap-3 text-sm text-blue-50">
        <span>Max buffer (ms)</span>
        <input
          type="number"
          min={0}
          max={5000}
          step={50}
          placeholder="∞"
          value={Number.isFinite(maximumMs) ? maximumMs : ''}
          onChange={(event) =>
            changePolicy({ maximumMs: nonNegativeNumber(event.target.value, Number.POSITIVE_INFINITY) })
          }
          className={CONTROL_CLASS}
        />
      </label>
      <label className="flex items-center justify-between gap-3 text-sm text-blue-50">
        <span>Catch up</span>
        <select
          value={settings.catchUp}
          onChange={(event) => onChange({ ...settings, catchUp: event.target.value as CatchUp })}
          className={CONTROL_CLASS}
        >
          {CATCH_UP_OPTIONS.map((option) => (
            <option key={option.value} value={option.value}>
              {option.label}
            </option>
          ))}
        </select>
      </label>
    </div>
  )
}

function nonNegativeNumber(value: string, fallback: number): number {
  const parsed = Number(value)
  return value.trim() !== '' && Number.isFinite(parsed) && parsed >= 0 ? parsed : fallback
}
