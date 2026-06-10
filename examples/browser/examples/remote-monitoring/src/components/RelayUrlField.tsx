import { RELAY_PRESETS } from '../types/monitoring'
import { Input } from './ui/input'
import { Label } from './ui/label'

interface Props {
  value: string
  onChange: (value: string) => void
}

export function RelayUrlField({ value, onChange }: Props) {
  return (
    <div className="space-y-1">
      <Label className="text-xs text-zinc-400 font-mono">Relay URL</Label>
      <div className="flex gap-1.5 mb-1.5">
        {RELAY_PRESETS.map((p) => (
          <button
            key={p.value}
            type="button"
            onClick={() => onChange(p.value)}
            className={`px-2.5 py-1 rounded text-xs font-mono border transition-colors ${
              value === p.value
                ? 'bg-blue-600 border-blue-500 text-white'
                : 'bg-zinc-800 border-zinc-700 text-zinc-400 hover:border-zinc-500'
            }`}
          >
            {p.label}
          </button>
        ))}
      </div>
      <Input
        value={value}
        onChange={(e) => onChange(e.target.value)}
        className="bg-zinc-800 border-zinc-700 font-mono text-sm"
      />
    </div>
  )
}
