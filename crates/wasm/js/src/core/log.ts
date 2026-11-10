import type { LogLevel, LogRecord } from './init'

type Fields = LogRecord['fields']

const SEVERITY: Record<LogLevel, number> = { error: 0, warn: 1, info: 2, debug: 3, trace: 4 }

let sink: { emit: (record: LogRecord, token: undefined) => void; level: LogLevel } | undefined

/// Hands the SDK's TypeScript logs at `level` or above to `emit`.
export function setSdkLogSink(emit: (record: LogRecord, token: undefined) => void, level: LogLevel): void {
  sink = { emit, level }
}

/// Logs from the SDK's TypeScript: to the console, and to the sink `init` set. `target`
/// names the module (`moq-wasm::media::live_player`). Values that are not a string, number or
/// boolean go into `fields` as their message (errors) or JSON.
export function sdkLog(level: LogLevel, target: string, message: string, fields: Record<string, unknown> = {}): void {
  const values = toFields(fields)
  const text = [`[${target}] ${message}`, ...Object.entries(values).map(([key, value]) => `${key}=${value}`)]
  console[CONSOLE_METHOD[level]](text.join(' '))
  emit(level, target, message, values)
}

/// `sdkLog` without the console, for a log the SDK also hands to the page's own callback
/// (`LivePlayerCallbacks.onLog`), which shows it as the page chooses.
export function forwardSdkLog(
  level: LogLevel,
  target: string,
  message: string,
  fields: Record<string, unknown> = {}
): void {
  emit(level, target, message, toFields(fields))
}

/// Hands a page's own log to `init`'s `onLog` and `telemetry`, as `forwardSdkLog` does the SDK's,
/// at `logLevel` or above. `target` names the page. It is not written to the console.
export const reportLog = forwardSdkLog

function emit(level: LogLevel, target: string, message: string, fields: Fields): void {
  if (sink && SEVERITY[level] <= SEVERITY[sink.level]) {
    sink.emit({ timeMs: Date.now(), level, target, message, fields }, undefined)
  }
}

/// The message a worker posts for `sdkLog` on the main thread, which `init`'s sink lives on.
export type WorkerLogMessage = {
  type: 'log'
  level: LogLevel
  target: string
  message: string
  fields: Fields
}

/// `sdkLog` for a dedicated worker: posts the record to the page, which passes it to `sdkLog`.
export function workerLog(
  level: LogLevel,
  target: string,
  message: string,
  fields: Record<string, unknown> = {}
): void {
  const record: WorkerLogMessage = { type: 'log', level, target, message, fields: toFields(fields) }
  self.postMessage(record)
}

/// Passes a worker's `workerLog` to `sdkLog`. Returns whether `data` was one.
export function receiveWorkerLog(data: { type?: unknown }): boolean {
  if (data.type !== 'log') {
    return false
  }
  const { level, target, message, fields } = data as WorkerLogMessage
  sdkLog(level, target, message, fields)
  return true
}

const CONSOLE_METHOD = { error: 'error', warn: 'warn', info: 'info', debug: 'debug', trace: 'debug' } as const

function toFields(fields: Record<string, unknown>): Fields {
  return Object.fromEntries(Object.entries(fields).map(([key, value]) => [key, toField(value)]))
}

function toField(value: unknown): string | number | boolean {
  if (typeof value === 'string' || typeof value === 'number' || typeof value === 'boolean') {
    return value
  }
  if (typeof value === 'bigint') {
    return value.toString()
  }
  if (value instanceof Error) {
    return value.message
  }
  try {
    return JSON.stringify(value) ?? String(value)
  } catch {
    return String(value)
  }
}
