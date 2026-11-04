type MoqtModule = typeof import('../../pkg/moqt')

export type LogLevel = 'error' | 'warn' | 'info' | 'debug' | 'trace'

/// One `tracing` event from inside the wasm module.
export interface LogRecord {
  timeMs: number
  level: LogLevel
  target: string
  message: string
  fields: Record<string, string | number | boolean>
  /// Ids of the span the event was logged in, in the same form as `SpanRecord`. Set only with `onSpan`.
  traceId?: string
  spanId?: string
}

/// A closed INFO or higher `tracing` span from the wasm module. Ids are lowercase hex.
export interface SpanRecord {
  traceId: string
  spanId: string
  parentSpanId?: string
  name: string
  startTimeMs: number
  endTimeMs: number
  fields: Record<string, string | number | boolean>
}

export interface InitOptions {
  /// Overrides where the `.wasm` file is fetched from, for bundlers that do not
  /// rewrite `new URL('moqt_bg.wasm', import.meta.url)`.
  wasmUrl?: string | URL
  /// Receives the wasm module's logs. Without it, they are discarded. `token` is the one
  /// the record's `MoqSession` connected with (its provider's newest), `undefined` for a
  /// record of no session or of a session without one. Put it only in the telemetry
  /// report's `token`: the server stores a record's fields as they are.
  onLog?: (record: LogRecord, token: string | undefined) => void
  /// The lowest level passed to `onLog`. Defaults to `'warn'`.
  logLevel?: LogLevel
  /// Receives the wasm module's closed spans, with `token` as for `onLog`. Without it,
  /// they are discarded.
  onSpan?: (record: SpanRecord, token: string | undefined) => void
}

let loading: Promise<MoqtModule> | null = null

/// Loads the wasm module and, with `onLog` or `onSpan`, installs its log sink.
/// The sink is page-wide, so a second call with either rejects. Call it before the first
/// `MoqSession.connect` to override `wasmUrl`: the first load decides the URL.
export async function init(options: InitOptions = {}): Promise<void> {
  const moqt = await loadMoqt(options.wasmUrl)
  if (options.onLog || options.onSpan) {
    moqt.setLogSink(options.onLog ?? (() => {}), options.logLevel ?? 'warn', options.onSpan)
  }
}

/// Loads and instantiates the wasm package once per page. The generated `init`
/// only checks for an existing instance before its first `await`, so two
/// overlapping calls instantiate the module twice, and the glue code then routes
/// every object and callback the first instance created to the second instance.
export function loadMoqt(wasmUrl?: string | URL): Promise<MoqtModule> {
  loading ??= import('../../pkg/moqt')
    .then(async (moqt) => {
      await moqt.default({ module_or_path: wasmUrl })
      return moqt
    })
    .catch((error) => {
      loading = null
      throw error
    })
  return loading
}
