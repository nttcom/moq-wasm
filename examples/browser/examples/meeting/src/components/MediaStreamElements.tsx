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
  picture?: HTMLElement | null
  placeholder: ReactNode
  footer?: ReactNode
  /// Fills the parent instead of a 16:9 frame, without media controls.
  fill?: boolean
}

/// Shows a picture element the live pipeline owns (a `<video>` fed by a
/// MediaStreamTrackGenerator, or a `<canvas>` where the generator is missing).
export function PictureFrame({ picture, placeholder, footer, fill = false }: PictureFrameProps) {
  const frameRef = useRef<HTMLDivElement | null>(null)
  const [hasFirstFrame, setHasFirstFrame] = useState(false)

  useEffect(() => {
    const frame = frameRef.current
    if (!frame || !picture) {
      setHasFirstFrame(false)
      return
    }
    picture.classList.add('h-full', 'object-contain')
    frame.appendChild(picture)
    if (!(picture instanceof HTMLVideoElement)) {
      setHasFirstFrame(true)
      return () => {
        picture.remove()
      }
    }
    const video = picture
    setHasFirstFrame(video.readyState >= HTMLMediaElement.HAVE_CURRENT_DATA)
    const handleLoadedData = () => setHasFirstFrame(true)
    const handleEmptied = () => setHasFirstFrame(false)
    video.addEventListener('loadeddata', handleLoadedData)
    video.addEventListener('emptied', handleEmptied)
    return () => {
      video.removeEventListener('loadeddata', handleLoadedData)
      video.removeEventListener('emptied', handleEmptied)
      video.remove()
    }
  }, [picture])

  useEffect(() => {
    if (picture instanceof HTMLVideoElement) {
      picture.controls = !fill && hasFirstFrame
    }
  }, [picture, hasFirstFrame, fill])

  return (
    <div className={fill ? 'h-full w-full' : 'w-full'}>
      <div className={`relative w-full overflow-hidden bg-black ${fill ? 'h-full' : 'aspect-video rounded-lg'}`}>
        <div ref={frameRef} className="h-full w-full" />
        {!hasFirstFrame && (
          <div className="pointer-events-none absolute inset-0 flex items-center justify-center text-sm text-slate-300">
            {placeholder}
          </div>
        )}
      </div>
      {footer}
    </div>
  )
}
