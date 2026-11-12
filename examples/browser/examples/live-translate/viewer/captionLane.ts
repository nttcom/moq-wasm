const CAPTION_HOLD_MS = 4_000
const MAX_LINE_CHARS = 120

type Piece = { text: string; final: boolean }

/// The transcript of one language as a caption line: pieces of an utterance
/// accumulate into one line, and the line clears once pieces stop. The player
/// hands each piece over when the audio it was spoken with plays.
export class CaptionLane {
  private line = ''
  private clearTimer: ReturnType<typeof setTimeout> | undefined
  private closed = false

  constructor(private readonly onCaption: (text: string) => void) {}

  push(payload: string): void {
    const piece = parsePiece(payload)
    if (!piece || this.closed) {
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

  close(): void {
    this.closed = true
    clearTimeout(this.clearTimer)
    // Also when the line is empty: a finished utterance stays on screen until its hold ends.
    this.line = ''
    this.onCaption('')
  }
}

function parsePiece(payload: string): Piece | undefined {
  try {
    const parsed = JSON.parse(payload) as { text?: unknown; final?: unknown }
    return typeof parsed.text === 'string' ? { text: parsed.text, final: parsed.final === true } : undefined
  } catch {
    return undefined
  }
}
