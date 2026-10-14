import type { AudioLevel } from '../../../../utils/media/loc'

/// A member speaks while its audio carried voice activity this recently.
const SPEAKING_WINDOW_MS = 500
/// The focus stays on a speaker through pauses this short, so turn-taking
/// and breaths do not move it.
const SPEAKER_HOLD_MS = 1_500

type Voice = { lastVoiceAtMs: number; level: number }

/// Picks the member to focus from the LOC Audio Level of their audio
/// (draft-ietf-moq-loc-01 §2.3.3.1): the loudest member speaking takes the
/// focus once the current speaker has been quiet for `SPEAKER_HOLD_MS`, and
/// the last speaker keeps it while nobody speaks.
export class DominantSpeaker {
  private readonly voices = new Map<string, Voice>()
  private dominant: string | undefined

  observe(memberId: string, { level, voiceActivity }: AudioLevel, nowMs: number): void {
    if (voiceActivity) {
      this.voices.set(memberId, { lastVoiceAtMs: nowMs, level })
    }
  }

  speaking(nowMs: number): Set<string> {
    const speaking = new Set<string>()
    for (const [memberId, voice] of this.voices) {
      if (nowMs - voice.lastVoiceAtMs <= SPEAKING_WINDOW_MS) {
        speaking.add(memberId)
      }
    }
    return speaking
  }

  current(nowMs: number): string | undefined {
    const dominantVoice = this.dominant === undefined ? undefined : this.voices.get(this.dominant)
    if (dominantVoice && nowMs - dominantVoice.lastVoiceAtMs < SPEAKER_HOLD_MS) {
      return this.dominant
    }
    let loudest: [string, Voice] | undefined
    for (const memberId of this.speaking(nowMs)) {
      const voice = this.voices.get(memberId)
      if (voice && (!loudest || voice.level < loudest[1].level)) {
        loudest = [memberId, voice]
      }
    }
    this.dominant = loudest?.[0] ?? this.dominant
    return this.dominant
  }
}
