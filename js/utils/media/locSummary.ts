export type LocHeaderSummary = {
  present: boolean
  extensionCount: number
  hasCaptureTimestamp: boolean
  hasVideoConfig: boolean
  hasVideoFrameMarking: boolean
  hasAudioLevel: boolean
}

export function summarizeLocHeader(locHeader: unknown): LocHeaderSummary {
  const extensions = Array.isArray((locHeader as { extensions?: unknown[] } | undefined)?.extensions)
    ? ((locHeader as { extensions: unknown[] }).extensions ?? [])
    : []
  const has = (type: string) =>
    extensions.some((ext) => typeof ext === 'object' && ext !== null && (ext as { type?: unknown }).type === type)
  return {
    present: Boolean(locHeader),
    extensionCount: extensions.length,
    hasCaptureTimestamp: has('captureTimestamp'),
    hasVideoConfig: has('videoConfig'),
    hasVideoFrameMarking: has('videoFrameMarking'),
    hasAudioLevel: has('audioLevel')
  }
}
