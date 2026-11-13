import { Input } from '../../../../lib/ui/input'
import { RelayPicker } from '../../../../lib/ui/relay-picker'

interface Props {
  value: string
  onChange: (value: string) => void
}

export function RelayUrlField({ value, onChange }: Props) {
  return (
    <div className="space-y-2">
      <RelayPicker value={value} onChange={onChange} className="text-zinc-300" />
      <Input
        value={value}
        onChange={(e) => onChange(e.target.value)}
        aria-label="Relay URL"
        className="bg-zinc-800 border-zinc-700 font-mono text-sm"
      />
    </div>
  )
}
