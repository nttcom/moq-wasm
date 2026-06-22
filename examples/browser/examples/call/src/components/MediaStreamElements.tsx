import { ReactNode, useEffect, useRef, useState } from 'react'
import { isCallVideoPipelineDebugEnabled } from '../utils/debug'

const VIDEO_ELEMENT_LOG_PREFIX = '[call][media-element][video]'

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
  if (!isCallVideoPipelineDebugEnabled()) {
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
      videoTrackIds: videoTracks.map((track) => track.id),
      readyState: element.readyState,
      networkState: element.networkState,
      paused: element.paused,
      currentTime: element.currentTime,
      error: element.error?.message ?? null
    })
  )
}

interface MediaStreamAudioProps {
  stream?: MediaStream | null
  className?: string
  testId?: string
}

export function MediaStreamAudio({ stream, className, testId }: MediaStreamAudioProps) {
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

  return <audio ref={ref} data-testid={testId} className={className} autoPlay controls={false} />
}
