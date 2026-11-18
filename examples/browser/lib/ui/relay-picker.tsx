import { type ReactNode, useEffect, useRef, useState } from 'react'
import { createPortal } from 'react-dom'
import { createRelayPicker, type RelayPicker as Picker } from '../../utils/relayPicker'

interface RelayPickerProps {
  value: string
  onChange: (url: string) => void
  className?: string
  /// Shown under the options while the picker is open, such as a URL input.
  children?: ReactNode
}

export function RelayPicker({ value, onChange, className, children }: RelayPickerProps) {
  const containerRef = useRef<HTMLDivElement>(null)
  const pickerRef = useRef<Picker | null>(null)
  const onChangeRef = useRef(onChange)
  onChangeRef.current = onChange
  const initialValueRef = useRef(value)
  const [extra] = useState(() => document.createElement('div'))

  useEffect(() => {
    const container = containerRef.current
    if (!container) return
    pickerRef.current = createRelayPicker(container, initialValueRef.current, (url) => onChangeRef.current(url), extra)
    return () => container.replaceChildren()
  }, [extra])

  useEffect(() => {
    pickerRef.current?.select(value)
  }, [value])

  return (
    <>
      <div ref={containerRef} className={className} />
      {children !== undefined && createPortal(children, extra)}
    </>
  )
}
