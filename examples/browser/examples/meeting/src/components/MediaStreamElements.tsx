import { ReactNode, useEffect, useRef, useState } from 'react'

interface MediaStreamVideoProps {
  stream?: MediaStream | null
  muted?: boolean
  footer?: ReactNode
  testId?: string
  /// Fills the parent instead of a 16:9 frame, without media controls.
  fill?: boolean
}

export function MediaStreamVideo({ stream, muted = false, footer, testId, fill = false }: MediaStreamVideoProps) {
  const ref = useRef<HTMLVideoElement | null>(null)
  const [hasFirstFrame, setHasFirstFrame] = useState(false)

  useEffect(() => {
    if (ref.current) {
      ref.current.srcObject = stream ?? null
    }
    setHasFirstFrame(false)
  }, [stream])

  return (
    <div className={fill ? 'h-full w-full' : 'w-full'}>
      <div className={`relative w-full overflow-hidden bg-black ${fill ? 'h-full' : 'aspect-video rounded-lg'}`}>
        <video
          ref={ref}
          data-testid={testId}
          className="w-full h-full object-contain"
          autoPlay
          playsInline
          muted={muted}
          controls={!fill && hasFirstFrame}
          onLoadedData={() => setHasFirstFrame(true)}
        />
      </div>
      {footer}
    </div>
  )
}

interface PictureFrameProps {
  /// The element a remote source's player draws into.
  element?: HTMLElement
  /// The player has shown a frame; the placeholder covers the element until then.
  shown: boolean
  placeholder: ReactNode
}

export function PictureFrame({ element, shown, placeholder }: PictureFrameProps) {
  const frameRef = useRef<HTMLDivElement | null>(null)

  useEffect(() => {
    const frame = frameRef.current
    if (!frame || !element) {
      return
    }
    frame.appendChild(element)
    return () => {
      element.remove()
    }
  }, [element])

  return (
    <div className="relative h-full w-full overflow-hidden bg-black">
      <div ref={frameRef} className="h-full w-full" />
      {!shown && (
        <div className="pointer-events-none absolute inset-0 flex items-center justify-center bg-slate-800 text-sm text-slate-300">
          {placeholder}
        </div>
      )}
    </div>
  )
}
