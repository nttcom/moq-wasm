import type { LivePlayout } from 'moq-wasm/media/player/livePlayout'

const CAPTION_HOLD_MS = 4_000
const MAX_LATE_MS = 3_000
const MAX_LINE_CHARS = 120

type Piece = { text: string; final: boolean }

/// The transcript of one language, shown on the live playout's clock: each
/// piece appears when the audio it was spoken with plays, pieces of an
/// utterance accumulate into one line, and the line clears once pieces stop.
export class CaptionLane {
  private line = ''
  private clearTimer: ReturnType<typeof setTimeout> | undefined
  private readonly timers = new Set<ReturnType<typeof setTimeout>>()
  private closed = false

  constructor(
    private readonly playout: Pick<LivePlayout, 'dueAt'>,
    private readonly onCaption: (text: string) => void
  ) {}

  push(payload: Uint8Array, captureMicros: number | undefined): void {
    const piece = parsePiece(payload)
    if (!piece || captureMicros === undefined) {
      return
    }
    const dueMs = this.playout.dueAt(captureMicros)
    const waitMs = dueMs === undefined ? 0 : dueMs - performance.now()
    if (waitMs < -MAX_LATE_MS) {
      return
    }
    const timer = setTimeout(
      () => {
        this.timers.delete(timer)
        this.show(piece)
      },
      Math.max(0, waitMs)
    )
    this.timers.add(timer)
  }

  close(): void {
    this.closed = true
    for (const timer of this.timers) {
      clearTimeout(timer)
    }
    this.timers.clear()
    clearTimeout(this.clearTimer)
    if (this.line) {
      this.line = ''
      this.onCaption('')
    }
  }

  private show(piece: Piece): void {
    if (this.closed) {
      return
    }
    this.line = (this.line + piece.text).slice(-MAX_LINE_CHARS)
    this.onCaption(this.line)
    clearTimeout(this.clearTimer)
    this.clearTimer = setTimeout(() => {
      this.line = ''
      this.onCaption('')
    }, CAPTION_HOLD_MS)
    if (piece.final) {
      // The next utterance starts a new line; the finished one stays until the hold ends.
      this.line = ''
    }
  }
}

function parsePiece(payload: Uint8Array): Piece | undefined {
  try {
    const parsed = JSON.parse(new TextDecoder().decode(payload)) as { text?: unknown; final?: unknown }
    return typeof parsed.text === 'string' ? { text: parsed.text, final: parsed.final === true } : undefined
  } catch {
    return undefined
  }
}
