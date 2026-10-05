import { useEffect, useRef } from 'react'

/// Places a camera player's host element in the tile that renders it. The host
/// outlives the tile, so a camera moving between the stage and the strip keeps
/// its picture.
export function PlayerHost({ host }: { host: HTMLElement }) {
  const slotRef = useRef<HTMLDivElement | null>(null)

  useEffect(() => {
    const slot = slotRef.current
    if (!slot) {
      return
    }
    slot.appendChild(host)
    return () => {
      if (host.parentElement === slot) {
        slot.removeChild(host)
      }
    }
  }, [host])

  return <div ref={slotRef} className="absolute inset-0" />
}
