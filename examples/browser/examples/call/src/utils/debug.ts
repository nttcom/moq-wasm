const DEBUG_VIDEO_PIPELINE_PARAM = 'debugVideoPipeline'

export function isCallVideoPipelineDebugEnabled(): boolean {
  if (typeof window === 'undefined') {
    return false
  }
  const value = new URLSearchParams(window.location.search).get(DEBUG_VIDEO_PIPELINE_PARAM)
  return value === '1' || value === 'true'
}
