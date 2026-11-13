import { useEffect, useRef } from 'react'
import { createRelayPicker, type RelayPicker as Picker } from '../../utils/relayPicker'

interface RelayPickerProps {
  value: string
  onChange: (url: string) => void
  className?: string
}

export function RelayPicker({ value, onChange, className }: RelayPickerProps) {
  const containerRef = useRef<HTMLDivElement>(null)
  const pickerRef = useRef<Picker | null>(null)
  const onChangeRef = useRef(onChange)
  onChangeRef.current = onChange
  const initialValueRef = useRef(value)

  useEffect(() => {
    const container = containerRef.current
    if (!container) return
    pickerRef.current = createRelayPicker(container, initialValueRef.current, (url) => onChangeRef.current(url))
    return () => container.replaceChildren()
  }, [])

  useEffect(() => {
    pickerRef.current?.select(value)
  }, [value])

  return <div ref={containerRef} className={className} />
}
