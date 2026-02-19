import { ReactNode, useEffect, useRef, useState } from 'react'

interface MediaStreamVideoProps {
  stream?: MediaStream | null
  muted?: boolean
  className?: string
  placeholder?: string
  overlay?: ReactNode
  footer?: ReactNode
}

export function MediaStreamVideo({
  stream,
  muted = false,
  className,
  placeholder = 'Video unavailable',
  overlay,
  footer
}: MediaStreamVideoProps) {
  const ref = useRef<HTMLVideoElement | null>(null)
  const [hasFirstFrame, setHasFirstFrame] = useState(false)

  useEffect(() => {
    if (ref.current) {
      ref.current.srcObject = stream ?? null
    }
    setHasFirstFrame(false)
  }, [stream])

  return (
    <div className="w-full">
      <div className={`relative w-full aspect-video overflow-hidden rounded-lg bg-black ${className}`}>
        <video
          ref={ref}
          className="w-full h-full object-contain"
          autoPlay
          playsInline
          muted={muted}
          controls={hasFirstFrame}
          onLoadedData={() => setHasFirstFrame(true)}
        />
        {overlay && (
          <div className="pointer-events-none absolute right-3 top-3 rounded-md bg-black/70 px-3 py-2 text-sm font-semibold text-white shadow-md">
            {overlay}
          </div>
        )}
      </div>
      {footer}
    </div>
  )
}

interface MediaStreamAudioProps {
  stream?: MediaStream | null
  className?: string
}

export function MediaStreamAudio({ stream, className }: MediaStreamAudioProps) {
  const ref = useRef<HTMLAudioElement | null>(null)

  useEffect(() => {
    const element = ref.current
    if (!element) {
      return
    }
    element.pause()
    element.srcObject = null
    if (stream) {
      element.srcObject = stream
      void element.play().catch(() => {})
    }
    return () => {
      element.pause()
      element.srcObject = null
    }
  }, [stream])

  return <audio ref={ref} className={className} autoPlay controls={false} />
}
