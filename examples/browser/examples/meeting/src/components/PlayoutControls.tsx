import type { CatchUp } from 'moq-wasm/media'
import { DEFAULT_PLAYOUT_SETTINGS, type PlayoutSettings } from '../types/playout'
import { FIELD_CLASS } from '../../../../lib/ui/field'

const CATCH_UP_OPTIONS: { value: CatchUp; label: string }[] = [
  { value: 'speed-up', label: '早回し (WSOLA)' },
  { value: 'skip', label: 'スキップ (trim + crossfade)' },
  { value: 'off', label: 'オフ' }
]

const CONTROL_CLASS = `w-40 ${FIELD_CLASS}`

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
    <div className="space-y-3 rounded-lg border border-line bg-subtle p-4">
      <div className="text-sm font-semibold text-fg">再生バッファ</div>
      <label className="flex items-center justify-between gap-3 text-sm text-fg">
        <span>最小バッファ (ms)</span>
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
      <label className="flex items-center justify-between gap-3 text-sm text-fg">
        <span>最大バッファ (ms)</span>
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
      <label className="flex items-center justify-between gap-3 text-sm text-fg">
        <span>追いつき方</span>
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
