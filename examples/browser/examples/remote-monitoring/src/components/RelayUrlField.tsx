import { Input } from '../../../../lib/ui/input'
import { RelayPicker } from '../../../../lib/ui/relay-picker'

interface Props {
  value: string
  onChange: (value: string) => void
}

export function RelayUrlField({ value, onChange }: Props) {
  return (
    <RelayPicker value={value} onChange={onChange} className="text-fg">
      <Input
        value={value}
        onChange={(e) => onChange(e.target.value)}
        aria-label="Relay URL"
        className="font-mono text-sm"
      />
    </RelayPicker>
  )
}
