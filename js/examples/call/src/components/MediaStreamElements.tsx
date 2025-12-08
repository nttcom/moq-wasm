import { ReactNode, useEffect, useRef } from 'react'

interface MediaStreamVideoProps {
  stream?: MediaStream | null
  muted?: boolean
  className?: string
  placeholder?: string
  overlay?: ReactNode
}

export function MediaStreamVideo({
  stream,
  muted = false,
  className,
  placeholder = 'Video unavailable',
  overlay
}: MediaStreamVideoProps) {
  const ref = useRef<HTMLVideoElement | null>(null)

  useEffect(() => {
    if (ref.current) {
      ref.current.srcObject = stream ?? null
    }
  }, [stream])

  return (
    <div className={`relative w-full aspect-video overflow-hidden rounded-lg bg-black ${className}`}>
      <video ref={ref} className="w-full h-full object-contain" autoPlay playsInline muted={muted} controls />
      {overlay && (
        <div className="pointer-events-none absolute right-3 top-3 rounded-md bg-black/70 px-3 py-2 text-sm font-semibold text-white shadow-md">
          {overlay}
        </div>
      )}
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
    if (ref.current) {
      ref.current.srcObject = stream ?? null
    }
  }, [stream])

  return <audio ref={ref} className={className} autoPlay controls={false} />
}
