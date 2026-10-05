import { ReactNode, useEffect, useRef, useState } from 'react'
import { isMeetingVideoPipelineDebugEnabled } from '../utils/debug'

const VIDEO_ELEMENT_LOG_PREFIX = '[meeting][media-element][video]'

interface MediaStreamVideoProps {
  stream?: MediaStream | null
  muted?: boolean
  className?: string
  placeholder?: string
  overlay?: ReactNode
  footer?: ReactNode
  testId?: string
}

export function MediaStreamVideo({
  stream,
  muted = false,
  className,
  placeholder = 'Video unavailable',
  overlay,
  footer,
  testId
}: MediaStreamVideoProps) {
  const ref = useRef<HTMLVideoElement | null>(null)
  const [hasFirstFrame, setHasFirstFrame] = useState(false)

  useEffect(() => {
    if (ref.current) {
      ref.current.srcObject = stream ?? null
      logVideoElementEvent(ref.current, 'src-object', testId, stream)
    }
    setHasFirstFrame(false)
  }, [stream, testId])

  const handleVideoEvent = (event: string) => {
    if (!ref.current) {
      return
    }
    logVideoElementEvent(ref.current, event, testId, stream)
    if (event === 'loadeddata') {
      setHasFirstFrame(true)
    }
  }

  return (
    <div className="w-full">
      <div className={`relative w-full aspect-video overflow-hidden rounded-lg bg-black ${className}`}>
        <video
          ref={ref}
          data-testid={testId}
          className="w-full h-full object-contain"
          autoPlay
          playsInline
          muted={muted}
          controls={hasFirstFrame}
          onLoadedMetadata={() => handleVideoEvent('loadedmetadata')}
          onLoadedData={() => handleVideoEvent('loadeddata')}
          onCanPlay={() => handleVideoEvent('canplay')}
          onPlaying={() => handleVideoEvent('playing')}
          onWaiting={() => handleVideoEvent('waiting')}
          onStalled={() => handleVideoEvent('stalled')}
          onError={() => handleVideoEvent('error')}
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

function logVideoElementEvent(
  element: HTMLVideoElement,
  event: string,
  testId: string | undefined,
  stream?: MediaStream | null
): void {
  if (!isMeetingVideoPipelineDebugEnabled()) {
    return
  }
  const videoTracks = stream?.getVideoTracks() ?? []
  console.info(
    VIDEO_ELEMENT_LOG_PREFIX,
    JSON.stringify({
      event,
      testId,
      hasStream: Boolean(stream),
      videoTrackCount: videoTracks.length,
      readyState: element.readyState,
      networkState: element.networkState,
      paused: element.paused,
      currentTime: element.currentTime,
      error: element.error?.message ?? null
    })
  )
}

interface PictureFrameProps {
  picture?: HTMLElement | null
  placeholder: string
  footer?: ReactNode
}

/// Shows a picture element the live pipeline owns (a `<video>` fed by a
/// MediaStreamTrackGenerator, or a `<canvas>` where the generator is missing).
export function PictureFrame({ picture, placeholder, footer }: PictureFrameProps) {
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
      picture.controls = hasFirstFrame
    }
  }, [picture, hasFirstFrame])

  return (
    <div className="w-full">
      <div className="relative w-full aspect-video overflow-hidden rounded-lg bg-black">
        <div ref={frameRef} className="h-full w-full" />
        {!hasFirstFrame && (
          <div className="pointer-events-none absolute inset-0 flex items-center justify-center text-sm text-blue-200/70">
            {placeholder}
          </div>
        )}
      </div>
      {footer}
    </div>
  )
}
