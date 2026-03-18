import {
  DEFAULT_VIDEO_ENCODING_SETTINGS,
  VIDEO_BITRATE_OPTIONS,
  VIDEO_CODEC_OPTIONS,
  VIDEO_HARDWARE_ACCELERATION_OPTIONS,
  VIDEO_RESOLUTION_OPTIONS
} from '../call/src/types/videoEncoding'

type DecoderHwa = 'prefer-hardware' | 'prefer-software'
type AvcFormatMode = 'auto' | 'annexb' | 'avc'

type LoopbackState = {
  runId: number
  stream: MediaStream | null
  sourceTrack: MediaStreamTrack | null
  processorTrack: MediaStreamTrack | null
  reader: ReadableStreamDefaultReader<VideoFrame> | null
  generator: MediaStreamTrackGenerator | null
  writer: WritableStreamDefaultWriter<VideoFrame> | null
  writerChain: Promise<void>
  encoder: VideoEncoder | null
  decoder: VideoDecoder | null
  decoderConfigured: boolean
  decoderConfigKey: string | null
  frameIndex: number
  inputEnded: boolean
  statsTimer: number | null
  lastMetadataOutputAtMs: number | null
  metadataSequence: number
}

type Stats = {
  startWallMs: number | null
  encodedFrames: number
  decodedFrames: number
  keyframes: number
  encodeErrors: number
  decodeErrors: number
  encoderQueueCurrent: number
  encoderQueueMax: number
  decoderQueueCurrent: number
  decoderQueueMax: number
  captureToEncodeDoneSamples: number[]
  encodeToDecodeDoneSamples: number[]
  captureToDecodeDoneSamples: number[]
  lastDecoderError?: string
}

const MAX_SAMPLE_HISTORY = 120
const MAX_LOG_LINES = 500

const state: LoopbackState = {
  runId: 0,
  stream: null,
  sourceTrack: null,
  processorTrack: null,
  reader: null,
  generator: null,
  writer: null,
  writerChain: Promise.resolve(),
  encoder: null,
  decoder: null,
  decoderConfigured: false,
  decoderConfigKey: null,
  frameIndex: 0,
  inputEnded: false,
  statsTimer: null,
  lastMetadataOutputAtMs: null,
  metadataSequence: 0
}

const stats: Stats = {
  startWallMs: null,
  encodedFrames: 0,
  decodedFrames: 0,
  keyframes: 0,
  encodeErrors: 0,
  decodeErrors: 0,
  encoderQueueCurrent: 0,
  encoderQueueMax: 0,
  decoderQueueCurrent: 0,
  decoderQueueMax: 0,
  captureToEncodeDoneSamples: [],
  encodeToDecodeDoneSamples: [],
  captureToDecodeDoneSamples: []
}

const captureTimestampByChunkTimestamp = new Map<number, number>()
const encodeDoneTimestampByChunkTimestamp = new Map<number, number>()

const sourceVideo = getEl<HTMLVideoElement>('sourceVideo')
const decodedVideo = getEl<HTMLVideoElement>('decodedVideo')
const codecSelect = getEl<HTMLSelectElement>('codecSelect')
const resolutionSelect = getEl<HTMLSelectElement>('resolutionSelect')
const bitrateSelect = getEl<HTMLSelectElement>('bitrateSelect')
const framerateInput = getEl<HTMLInputElement>('framerateInput')
const keyframeIntervalInput = getEl<HTMLInputElement>('keyframeIntervalInput')
const encoderHwaSelect = getEl<HTMLSelectElement>('encoderHwaSelect')
const encoderRealtimeCheckbox = getEl<HTMLInputElement>('encoderRealtimeCheckbox')
const decoderHwaSelect = getEl<HTMLSelectElement>('decoderHwaSelect')
const decoderAvcFormatSelect = getEl<HTMLSelectElement>('decoderAvcFormatSelect')
const decoderOptimizeLatencyCheckbox = getEl<HTMLInputElement>('decoderOptimizeLatencyCheckbox')
const checkSupportBtn = getEl<HTMLButtonElement>('checkSupportBtn')
const startBtn = getEl<HTMLButtonElement>('startBtn')
const stopBtn = getEl<HTMLButtonElement>('stopBtn')
const clearLogBtn = getEl<HTMLButtonElement>('clearLogBtn')
const statusText = getEl<HTMLDivElement>('statusText')
const supportText = getEl<HTMLDivElement>('supportText')
const statsGrid = getEl<HTMLDivElement>('statsGrid')
const logArea = getEl<HTMLTextAreaElement>('logArea')

initUi()
attachHandlers()
renderStats()

function initUi(): void {
  for (const option of VIDEO_CODEC_OPTIONS) {
    const el = document.createElement('option')
    el.value = option.codec
    el.textContent = option.label
    codecSelect.append(el)
  }
  for (const option of VIDEO_RESOLUTION_OPTIONS) {
    const el = document.createElement('option')
    el.value = `${option.width}x${option.height}`
    el.textContent = option.label
    resolutionSelect.append(el)
  }
  for (const option of VIDEO_BITRATE_OPTIONS) {
    const el = document.createElement('option')
    el.value = String(option.bitrate)
    el.textContent = option.label
    bitrateSelect.append(el)
  }
  for (const option of VIDEO_HARDWARE_ACCELERATION_OPTIONS) {
    const el = document.createElement('option')
    el.value = option.value
    el.textContent = option.value
    encoderHwaSelect.append(el)
  }

  codecSelect.value = DEFAULT_VIDEO_ENCODING_SETTINGS.codec
  resolutionSelect.value = `${DEFAULT_VIDEO_ENCODING_SETTINGS.width}x${DEFAULT_VIDEO_ENCODING_SETTINGS.height}`
  bitrateSelect.value = String(DEFAULT_VIDEO_ENCODING_SETTINGS.bitrate)
  framerateInput.value = String(DEFAULT_VIDEO_ENCODING_SETTINGS.framerate)
  keyframeIntervalInput.value = '150'
  encoderHwaSelect.value = DEFAULT_VIDEO_ENCODING_SETTINGS.hardwareAcceleration
  decoderHwaSelect.value = 'prefer-hardware'
  decoderAvcFormatSelect.value = 'auto'

  filterResolutionOptionsForCodec()
}

function attachHandlers(): void {
  codecSelect.addEventListener('change', () => {
    filterResolutionOptionsForCodec()
  })

  checkSupportBtn.addEventListener('click', async () => {
    await checkConfigSupport()
  })

  startBtn.addEventListener('click', async () => {
    await startLoopback()
  })

  stopBtn.addEventListener('click', async () => {
    await stopLoopback('user-stop')
  })

  clearLogBtn.addEventListener('click', () => {
    logArea.value = ''
  })
}

function getEl<T extends HTMLElement>(id: string): T {
  const el = document.getElementById(id)
  if (!el) {
    throw new Error(`element not found: ${id}`)
  }
  return el as T
}

function log(message: string, detail?: unknown, level: 'info' | 'warn' | 'error' = 'info'): void {
  const ts = new Date().toLocaleTimeString('ja-JP', { hour12: false })
  const line =
    detail === undefined ? `[${ts}] ${message}` : `[${ts}] ${message} ${safeStringify(detail) ?? String(detail)}`
  const lines = (logArea.value ? logArea.value.split('\n') : []).concat(line)
  if (lines.length > MAX_LOG_LINES) {
    lines.splice(0, lines.length - MAX_LOG_LINES)
  }
  logArea.value = lines.join('\n')
  logArea.scrollTop = logArea.scrollHeight

  if (level === 'error') console.error(message, detail)
  else if (level === 'warn') console.warn(message, detail)
  else console.info(message, detail)
}

function safeStringify(value: unknown): string | null {
  try {
    return JSON.stringify(value)
  } catch {
    return null
  }
}

function setStatus(text: string): void {
  statusText.innerHTML = `<strong>Status:</strong> ${escapeHtml(text)}`
}

function setSupportText(text: string, level: 'normal' | 'warn' | 'danger' = 'normal'): void {
  supportText.textContent = text
  supportText.className =
    level === 'danger' ? 'status-line danger-text' : level === 'warn' ? 'status-line warn-text' : 'status-line'
}

function escapeHtml(text: string): string {
  return text.replaceAll('&', '&amp;').replaceAll('<', '&lt;').replaceAll('>', '&gt;')
}

function filterResolutionOptionsForCodec(): void {
  const codec = codecSelect.value
  const codecOption = VIDEO_CODEC_OPTIONS.find((c) => c.codec === codec)
  const previous = resolutionSelect.value

  for (const optionEl of Array.from(resolutionSelect.options)) {
    const [wText, hText] = optionEl.value.split('x')
    const width = Number(wText)
    const height = Number(hText)
    const pixels = width * height
    optionEl.hidden = typeof codecOption?.maxEncodePixels === 'number' && pixels > codecOption.maxEncodePixels
  }

  const selectedOption = Array.from(resolutionSelect.options).find((o) => o.value === previous && !o.hidden)
  if (selectedOption) {
    resolutionSelect.value = previous
    return
  }

  const fallback = Array.from(resolutionSelect.options).find((o) => !o.hidden)
  if (fallback) {
    resolutionSelect.value = fallback.value
  }
}

function getSelectedResolution(): { width: number; height: number } {
  const [w, h] = resolutionSelect.value.split('x').map((v) => Number(v))
  return { width: Math.max(1, w || 1), height: Math.max(1, h || 1) }
}

function getEncoderConfig(): VideoEncoderConfig {
  const { width, height } = getSelectedResolution()
  const codec = codecSelect.value
  const bitrate = clampInt(Number(bitrateSelect.value), 1, 100_000_000, DEFAULT_VIDEO_ENCODING_SETTINGS.bitrate)
  const framerate = clampInt(Number(framerateInput.value), 1, 120, DEFAULT_VIDEO_ENCODING_SETTINGS.framerate)
  const hardwareAcceleration = (encoderHwaSelect.value as HardwareAcceleration) ?? 'prefer-hardware'
  const latencyMode = encoderRealtimeCheckbox.checked ? ('realtime' as any) : undefined

  return {
    codec,
    width,
    height,
    bitrate,
    framerate,
    hardwareAcceleration,
    latencyMode,
    avc: codec.startsWith('avc') ? ({ format: 'annexb' } as any) : undefined,
    scalabilityMode: 'L1T1'
  }
}

function getDecoderConfigBase(): {
  hardwareAcceleration: DecoderHwa
  optimizeForLatency: boolean
  avcFormatMode: AvcFormatMode
} {
  return {
    hardwareAcceleration: decoderHwaSelect.value as DecoderHwa,
    optimizeForLatency: decoderOptimizeLatencyCheckbox.checked,
    avcFormatMode: decoderAvcFormatSelect.value as AvcFormatMode
  }
}

function buildDecoderConfigFromCodec(
  codec: string,
  decoderConfigFromEncoder?: VideoDecoderConfig | (EncodedVideoChunkMetadata['decoderConfig'] & Record<string, unknown>)
): VideoDecoderConfig {
  const base = getDecoderConfigBase()
  const encoderCfg = (decoderConfigFromEncoder ?? {}) as {
    codec?: string
    description?: BufferSource
    avc?: { format?: 'annexb' | 'avc' }
  }
  const resolvedCodec = encoderCfg.codec ?? codec
  const config: VideoDecoderConfig = {
    codec: resolvedCodec,
    hardwareAcceleration: base.hardwareAcceleration as any,
    optimizeForLatency: base.optimizeForLatency
  }
  if (encoderCfg.description) {
    config.description = toUint8Array(encoderCfg.description)
  }
  if (resolvedCodec.startsWith('avc')) {
    const format =
      base.avcFormatMode === 'auto' ? (encoderCfg.avc?.format ?? 'annexb') : (base.avcFormatMode as 'annexb' | 'avc')
    ;(config as any).avc = { format }
  }
  return config
}

function toUint8Array(value: BufferSource): Uint8Array {
  if (value instanceof Uint8Array) {
    return new Uint8Array(value)
  }
  if (value instanceof ArrayBuffer) {
    return new Uint8Array(value.slice(0))
  }
  return new Uint8Array(value.buffer.slice(value.byteOffset, value.byteOffset + value.byteLength))
}

function clampInt(value: number, min: number, max: number, fallback: number): number {
  if (!Number.isFinite(value)) return fallback
  return Math.min(max, Math.max(min, Math.round(value)))
}

async function checkConfigSupport(): Promise<void> {
  const encoderConfig = getEncoderConfig()
  const decoderProbeConfig = buildDecoderConfigFromCodec(encoderConfig.codec)

  try {
    const [enc, dec] = await Promise.all([
      VideoEncoder.isConfigSupported(encoderConfig),
      VideoDecoder.isConfigSupported(decoderProbeConfig)
    ])
    const status = `Encoder supported=${enc.supported}, Decoder supported=${dec.supported}`
    setSupportText(status, enc.supported && dec.supported ? 'normal' : 'warn')
    log('[check] support', { encoder: enc, decoder: dec })
  } catch (error) {
    setSupportText('Config check failed', 'danger')
    log('[check] support failed', { error: String(error) }, 'error')
  }
}

async function startLoopback(): Promise<void> {
  startBtn.disabled = true
  try {
    await stopLoopback('restart')
    resetStats()
    stats.startWallMs = performance.now()

    const encoderConfig = getEncoderConfig()
    const keyframeInterval = clampInt(Number(keyframeIntervalInput.value), 1, 10_000, 150)
    keyframeIntervalInput.value = String(keyframeInterval)

    const stream = await navigator.mediaDevices.getUserMedia({
      video: {
        width: { ideal: encoderConfig.width },
        height: { ideal: encoderConfig.height },
        frameRate: { ideal: encoderConfig.framerate }
      },
      audio: false
    })
    const sourceTrack = stream.getVideoTracks()[0] ?? null
    if (!sourceTrack) {
      throw new Error('camera track not available')
    }
    sourceVideo.srcObject = stream

    const processorTrack = sourceTrack.clone()
    const processor = new MediaStreamTrackProcessor({ track: processorTrack })
    const reader = processor.readable.getReader()

    const generator = new MediaStreamTrackGenerator({ kind: 'video' })
    const writer = generator.writable.getWriter()
    decodedVideo.srcObject = new MediaStream([generator])

    const runId = state.runId + 1
    state.runId = runId
    state.stream = stream
    state.sourceTrack = sourceTrack
    state.processorTrack = processorTrack
    state.reader = reader
    state.generator = generator
    state.writer = writer
    state.writerChain = Promise.resolve()
    state.frameIndex = 0
    state.inputEnded = false
    state.decoderConfigured = false
    state.decoderConfigKey = null
    state.lastMetadataOutputAtMs = null
    state.metadataSequence = 0

    const decoder = new VideoDecoder({
      output: (frame) => {
        const outputAt = performance.now()
        stats.decodedFrames += 1
        stats.decoderQueueCurrent = decoder.decodeQueueSize
        stats.decoderQueueMax = Math.max(stats.decoderQueueMax, decoder.decodeQueueSize)

        const encodeDoneAt = encodeDoneTimestampByChunkTimestamp.get(frame.timestamp)
        if (typeof encodeDoneAt === 'number') {
          pushSample(stats.encodeToDecodeDoneSamples, outputAt - encodeDoneAt)
          encodeDoneTimestampByChunkTimestamp.delete(frame.timestamp)
        }
        const capturedAt = captureTimestampByChunkTimestamp.get(frame.timestamp)
        if (typeof capturedAt === 'number') {
          pushSample(stats.captureToDecodeDoneSamples, outputAt - capturedAt)
          captureTimestampByChunkTimestamp.delete(frame.timestamp)
        }

        state.writerChain = state.writerChain
          .then(async () => {
            if (state.runId !== runId || !state.writer) {
              return
            }
            await state.writer.write(frame)
          })
          .catch((error) => {
            log('[decoder] writer error', { error: String(error) }, 'error')
          })
          .finally(() => {
            frame.close()
          })
      },
      error: (error) => {
        stats.decodeErrors += 1
        stats.lastDecoderError = String(error)
        log('[decoder] error', { error: String(error) }, 'error')
      }
    })
    state.decoder = decoder

    const encoder = new VideoEncoder({
      output: (chunk, metadata) => {
        const outputAt = performance.now()
        stats.encodedFrames += 1
        if (chunk.type === 'key') {
          stats.keyframes += 1
        }
        stats.encoderQueueCurrent = encoder.encodeQueueSize
        stats.encoderQueueMax = Math.max(stats.encoderQueueMax, encoder.encodeQueueSize)

        const metadataDecoderConfig = metadata?.decoderConfig as
          | {
              codec?: string
              description?: BufferSource
              avc?: { format?: 'annexb' | 'avc' }
            }
          | undefined
        if (metadataDecoderConfig) {
          const previousAt = state.lastMetadataOutputAtMs
          const intervalMs = previousAt === null ? null : Math.max(0, outputAt - previousAt)
          state.lastMetadataOutputAtMs = outputAt
          state.metadataSequence += 1
          const descriptionBytes = metadataDecoderConfig.description
            ? toUint8Array(metadataDecoderConfig.description).byteLength
            : undefined
          log('[encoder] metadata.decoderConfig', {
            seq: state.metadataSequence,
            frameIndex: state.frameIndex - 1,
            chunkType: chunk.type,
            timestamp: chunk.timestamp,
            intervalMs: intervalMs === null ? null : Number(intervalMs.toFixed(2)),
            codec: metadataDecoderConfig.codec,
            avcFormat: metadataDecoderConfig.avc?.format,
            descriptionBytes
          })
        }

        const capturedAt = captureTimestampByChunkTimestamp.get(chunk.timestamp)
        if (typeof capturedAt === 'number') {
          pushSample(stats.captureToEncodeDoneSamples, outputAt - capturedAt)
        }
        encodeDoneTimestampByChunkTimestamp.set(chunk.timestamp, outputAt)
        trimMap(encodeDoneTimestampByChunkTimestamp, 4096)

        const decoderConfig = buildDecoderConfigFromCodec(encoderConfig.codec, metadataDecoderConfig as any)
        const configKey = buildDecoderConfigKey(decoderConfig)
        if (!state.decoderConfigured || state.decoderConfigKey !== configKey) {
          try {
            decoder.configure(decoderConfig)
            state.decoderConfigured = true
            state.decoderConfigKey = configKey
            log('[decoder] configured', {
              codec: decoderConfig.codec,
              avc: (decoderConfig as any).avc,
              hardwareAcceleration: (decoderConfig as any).hardwareAcceleration,
              optimizeForLatency: (decoderConfig as any).optimizeForLatency,
              descriptionBytes:
                decoderConfig.description instanceof Uint8Array ? decoderConfig.description.byteLength : undefined
            })
          } catch (error) {
            stats.decodeErrors += 1
            stats.lastDecoderError = String(error)
            log('[decoder] configure failed', { error: String(error), decoderConfig }, 'error')
            return
          }
        }

        try {
          decoder.decode(
            new EncodedVideoChunk({
              type: chunk.type,
              timestamp: chunk.timestamp,
              duration: chunk.duration ?? undefined,
              data: copyChunkData(chunk)
            })
          )
        } catch (error) {
          stats.decodeErrors += 1
          stats.lastDecoderError = String(error)
          log('[decoder] decode() threw', { error: String(error), chunkType: chunk.type }, 'error')
        }
      },
      error: (error) => {
        stats.encodeErrors += 1
        log('[encoder] error', { error: String(error) }, 'error')
      }
    })
    state.encoder = encoder
    encoder.configure(encoderConfig)
    log('[encoder] configured', {
      codec: encoderConfig.codec,
      width: encoderConfig.width,
      height: encoderConfig.height,
      bitrate: encoderConfig.bitrate,
      framerate: encoderConfig.framerate,
      hardwareAcceleration: encoderConfig.hardwareAcceleration,
      latencyMode: encoderConfig.latencyMode,
      avc: (encoderConfig as any).avc
    })

    state.statsTimer = window.setInterval(() => {
      if (state.encoder) {
        stats.encoderQueueCurrent = state.encoder.encodeQueueSize
      }
      if (state.decoder) {
        stats.decoderQueueCurrent = state.decoder.decodeQueueSize
      }
      renderStats()
    }, 500)

    setStatus('running')
    setSupportText('')
    stopBtn.disabled = false

    void pumpFrames(runId, encoder, reader, keyframeInterval)
  } catch (error) {
    log('[loopback] start failed', { error: String(error) }, 'error')
    setStatus('start failed')
    await stopLoopback('start-failed')
  } finally {
    startBtn.disabled = false
  }
}

async function pumpFrames(
  runId: number,
  encoder: VideoEncoder,
  reader: ReadableStreamDefaultReader<VideoFrame>,
  keyframeInterval: number
): Promise<void> {
  try {
    while (state.runId === runId) {
      const result = await reader.read()
      if (result.done) {
        state.inputEnded = true
        break
      }
      const frame = result.value
      const frameTimestamp = frame.timestamp
      captureTimestampByChunkTimestamp.set(frameTimestamp, performance.now())
      trimMap(captureTimestampByChunkTimestamp, 4096)
      const keyFrame = state.frameIndex % keyframeInterval === 0
      state.frameIndex += 1
      try {
        encoder.encode(frame, { keyFrame })
        stats.encoderQueueCurrent = encoder.encodeQueueSize
        stats.encoderQueueMax = Math.max(stats.encoderQueueMax, encoder.encodeQueueSize)
      } catch (error) {
        stats.encodeErrors += 1
        log('[encoder] encode() failed', { error: String(error), keyFrame }, 'error')
      } finally {
        frame.close()
      }
    }
  } catch (error) {
    if (state.runId === runId) {
      log('[loopback] reader loop failed', { error: String(error) }, 'error')
    }
  } finally {
    if (state.runId === runId) {
      await finalizePipeline(runId)
      setStatus('stopped')
      stopBtn.disabled = true
    }
  }
}

async function finalizePipeline(runId: number): Promise<void> {
  const encoder = state.encoder
  const decoder = state.decoder
  if (encoder && encoder.state !== 'closed') {
    try {
      await encoder.flush()
    } catch {
      // ignore
    }
    try {
      encoder.close()
    } catch {
      // ignore
    }
  }
  if (decoder && decoder.state !== 'closed') {
    try {
      await decoder.flush()
    } catch {
      // ignore
    }
    try {
      decoder.close()
    } catch {
      // ignore
    }
  }
  try {
    await state.writerChain
  } catch {
    // ignore
  }
  if (state.runId === runId && state.writer) {
    try {
      await state.writer.close()
    } catch {
      // ignore
    }
  }
}

async function stopLoopback(reason: string): Promise<void> {
  const previousRunId = state.runId
  state.runId += 1
  if (state.statsTimer !== null) {
    clearInterval(state.statsTimer)
    state.statsTimer = null
  }

  if (state.reader) {
    try {
      await state.reader.cancel(reason)
    } catch {
      // ignore
    }
    state.reader = null
  }

  if (state.encoder && state.encoder.state !== 'closed') {
    try {
      state.encoder.close()
    } catch {
      // ignore
    }
  }
  state.encoder = null

  if (state.decoder && state.decoder.state !== 'closed') {
    try {
      state.decoder.close()
    } catch {
      // ignore
    }
  }
  state.decoder = null
  state.decoderConfigured = false
  state.decoderConfigKey = null

  try {
    await state.writerChain
  } catch {
    // ignore
  }
  if (state.writer) {
    try {
      await state.writer.close()
    } catch {
      // ignore
    }
  }
  state.writer = null

  if (state.generator) {
    try {
      state.generator.stop()
    } catch {
      // ignore
    }
  }
  state.generator = null

  if (state.processorTrack) {
    try {
      state.processorTrack.stop()
    } catch {
      // ignore
    }
  }
  state.processorTrack = null

  if (state.stream) {
    for (const track of state.stream.getTracks()) {
      track.stop()
    }
  }
  state.stream = null
  state.sourceTrack = null
  sourceVideo.srcObject = null
  decodedVideo.srcObject = null

  captureTimestampByChunkTimestamp.clear()
  encodeDoneTimestampByChunkTimestamp.clear()
  state.writerChain = Promise.resolve()
  state.inputEnded = false
  state.frameIndex = 0

  stopBtn.disabled = true
  if (previousRunId !== 0) {
    log('[loopback] stopped', { reason })
  }
  renderStats()
}

function resetStats(): void {
  stats.startWallMs = null
  stats.encodedFrames = 0
  stats.decodedFrames = 0
  stats.keyframes = 0
  stats.encodeErrors = 0
  stats.decodeErrors = 0
  stats.encoderQueueCurrent = 0
  stats.encoderQueueMax = 0
  stats.decoderQueueCurrent = 0
  stats.decoderQueueMax = 0
  stats.captureToEncodeDoneSamples.length = 0
  stats.encodeToDecodeDoneSamples.length = 0
  stats.captureToDecodeDoneSamples.length = 0
  stats.lastDecoderError = undefined
  renderStats()
}

function renderStats(): void {
  const items: Array<{ label: string; value: string }> = [
    { label: 'Encoded Frames', value: String(stats.encodedFrames) },
    { label: 'Decoded Frames', value: String(stats.decodedFrames) },
    { label: 'Keyframes', value: String(stats.keyframes) },
    { label: 'Encoder Q (cur/max)', value: `${stats.encoderQueueCurrent} / ${stats.encoderQueueMax}` },
    { label: 'Decoder Q (cur/max)', value: `${stats.decoderQueueCurrent} / ${stats.decoderQueueMax}` },
    { label: 'Capture→EncodeDone avg', value: formatMs(average(stats.captureToEncodeDoneSamples)) },
    { label: 'EncodeDone→DecodeOut avg', value: formatMs(average(stats.encodeToDecodeDoneSamples)) },
    { label: 'Capture→DecodeOut avg', value: formatMs(average(stats.captureToDecodeDoneSamples)) },
    { label: 'Encoder Errors', value: String(stats.encodeErrors) },
    { label: 'Decoder Errors', value: String(stats.decodeErrors) },
    {
      label: 'Runtime',
      value: stats.startWallMs === null ? '-' : formatSeconds((performance.now() - stats.startWallMs) / 1000)
    },
    { label: 'Last Decoder Error', value: stats.lastDecoderError ?? '-' }
  ]

  statsGrid.innerHTML = ''
  for (const item of items) {
    const div = document.createElement('div')
    div.className = 'stat'
    div.innerHTML = `<div class="k">${escapeHtml(item.label)}</div><div class="v">${escapeHtml(item.value)}</div>`
    statsGrid.append(div)
  }
}

function average(values: number[]): number | null {
  if (!values.length) return null
  const sum = values.reduce((acc, value) => acc + value, 0)
  return sum / values.length
}

function pushSample(array: number[], value: number): void {
  if (!Number.isFinite(value)) return
  array.push(Math.max(0, value))
  if (array.length > MAX_SAMPLE_HISTORY) {
    array.splice(0, array.length - MAX_SAMPLE_HISTORY)
  }
}

function trimMap<T>(map: Map<number, T>, maxSize: number): void {
  while (map.size > maxSize) {
    const key = map.keys().next().value
    if (typeof key !== 'number') {
      break
    }
    map.delete(key)
  }
}

function formatMs(value: number | null): string {
  if (value === null) return '-'
  return `${value.toFixed(1)} ms`
}

function formatSeconds(value: number): string {
  return `${value.toFixed(1)} s`
}

function copyChunkData(chunk: EncodedVideoChunk): Uint8Array {
  const bytes = new Uint8Array(chunk.byteLength)
  chunk.copyTo(bytes)
  return bytes
}

function buildDecoderConfigKey(config: VideoDecoderConfig): string {
  const descriptionLength =
    config.description instanceof Uint8Array
      ? config.description.byteLength
      : config.description instanceof ArrayBuffer
        ? config.description.byteLength
        : config.description
          ? (config.description as ArrayBufferView).byteLength
          : 0
  return JSON.stringify({
    codec: config.codec,
    avc: (config as any).avc?.format,
    hwa: (config as any).hardwareAcceleration,
    optimizeForLatency: (config as any).optimizeForLatency,
    descriptionLength
  })
}
