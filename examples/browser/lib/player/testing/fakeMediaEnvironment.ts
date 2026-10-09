const MILLIS_PER_SECOND = 1_000
const MICROS_PER_SECOND = 1_000_000
const FAKED_GLOBALS = [
  'performance',
  'setTimeout',
  'clearTimeout',
  'AudioContext',
  'GainNode',
  'AudioBufferSourceNode'
] as const

type Timer = { id: number; dueMs: number; callback: () => void }

export class FakeMediaEnvironment {
  nowMs = 1_000
  private timers: Timer[] = []
  private nextTimerId = 1
  private readonly originals = new Map<string, PropertyDescriptor | undefined>()

  private constructor(private readonly outputLatencySeconds: number) {}

  static install({ outputLatencySeconds }: { outputLatencySeconds: number }): FakeMediaEnvironment {
    const environment = new FakeMediaEnvironment(outputLatencySeconds)
    const fakes = environment.fakeGlobals()
    for (const name of FAKED_GLOBALS) {
      environment.originals.set(name, Object.getOwnPropertyDescriptor(globalThis, name))
      Object.defineProperty(globalThis, name, { value: fakes[name], configurable: true, writable: true })
    }
    return environment
  }

  uninstall(): void {
    for (const [name, descriptor] of this.originals) {
      if (descriptor) {
        Object.defineProperty(globalThis, name, descriptor)
      } else {
        Reflect.deleteProperty(globalThis, name)
      }
    }
  }

  advanceTo(targetMs: number): void {
    for (let due = this.nextDueTimer(targetMs); due; due = this.nextDueTimer(targetMs)) {
      this.timers = this.timers.filter((timer) => timer !== due)
      this.nowMs = Math.max(this.nowMs, due.dueMs)
      due.callback()
    }
    this.nowMs = targetMs
  }

  private nextDueTimer(targetMs: number): Timer | undefined {
    const [earliest] = this.timers.toSorted((left, right) => left.dueMs - right.dueMs)
    return earliest && earliest.dueMs <= targetMs ? earliest : undefined
  }

  private fakeGlobals(): Record<(typeof FAKED_GLOBALS)[number], unknown> {
    const environment = this
    return {
      performance: { now: () => environment.nowMs },
      setTimeout: (callback: () => void, delayMs = 0) => {
        const id = environment.nextTimerId++
        environment.timers.push({ id, dueMs: environment.nowMs + delayMs, callback })
        return id
      },
      clearTimeout: (id: number | undefined) => {
        environment.timers = environment.timers.filter((timer) => timer.id !== id)
      },
      AudioContext: class {
        readonly state = 'running'
        readonly destination = {}
        readonly outputLatency = environment.outputLatencySeconds
        readonly sampleRate: number

        constructor({ sampleRate }: { sampleRate: number }) {
          this.sampleRate = sampleRate
        }

        get currentTime(): number {
          return environment.nowMs / MILLIS_PER_SECOND
        }

        createBuffer(_channelCount: number, length: number, sampleRate: number) {
          return { duration: length / sampleRate, copyToChannel() {} }
        }
      },
      GainNode: class {
        connect() {}
      },
      AudioBufferSourceNode: class {
        connect() {}
        addEventListener() {}
        start() {}
      }
    }
  }
}

export function fakeAudioData(sampleRate: number, numberOfFrames: number): AudioData {
  return {
    sampleRate,
    numberOfFrames,
    numberOfChannels: 1,
    duration: (numberOfFrames * MICROS_PER_SECOND) / sampleRate,
    copyTo() {},
    close() {}
  } as unknown as AudioData
}

export function fakeVideoFrame(timestamp: number): VideoFrame {
  return { timestamp, close() {} } as unknown as VideoFrame
}
