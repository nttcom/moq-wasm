import type { MoqSession } from '../../core/session'

const FAKED_GLOBALS = ['MediaStream', 'MediaStreamTrack', 'Worker', 'navigator'] as const

type Kind = 'video' | 'audio'

export class FakeTrack {
  stopped = false

  constructor(
    readonly kind: Kind,
    readonly clonedFrom?: FakeTrack
  ) {}

  stop(): void {
    this.stopped = true
  }

  clone(): FakeTrack {
    return new FakeTrack(this.kind, this)
  }

  private readonly endedListeners: (() => void)[] = []

  addEventListener(type: string, listener: () => void): void {
    if (type === 'ended') {
      this.endedListeners.push(listener)
    }
  }

  end(): void {
    for (const listener of this.endedListeners.splice(0)) {
      listener()
    }
  }
}

class FakeMediaStream {
  private readonly tracks: FakeTrack[]

  constructor(tracks: FakeTrack[] = []) {
    this.tracks = [...tracks]
  }

  getVideoTracks(): FakeTrack[] {
    return this.tracks.filter(({ kind }) => kind === 'video')
  }

  getAudioTracks(): FakeTrack[] {
    return this.tracks.filter(({ kind }) => kind === 'audio')
  }

  addTrack(track: FakeTrack): void {
    this.tracks.push(track)
  }

  removeTrack(track: FakeTrack): void {
    this.tracks.splice(this.tracks.indexOf(track), 1)
  }
}

type WorkerMessage = { type?: string; track?: FakeTrack; rendition?: number; wanted?: boolean }

class FakeWorker {
  readonly messages: WorkerMessage[] = []
  private terminated = false

  postMessage(message: WorkerMessage): void {
    this.messages.push(message)
  }

  terminate(): void {
    this.terminated = true
  }

  encodes(track: FakeTrack): boolean {
    return (
      !this.terminated &&
      !this.messages.some(({ type }) => type === 'stop') &&
      this.messages.some((message) => message.track?.clonedFrom === track)
    )
  }
}

class FakeMediaSource {
  removal: Promise<void> = Promise.resolve()

  removeTrack(): Promise<void> {
    return this.removal
  }
}

/// `declaring` is what `declareTracks` resolves with; `declareCalled`
/// resolves once it was called.
class FakeMediaBroadcast {
  readonly sources: FakeMediaSource[] = []
  declaring: Promise<void> = Promise.resolve()
  private readonly declared = Promise.withResolvers<void>()
  readonly declareCalled = this.declared.promise

  readonly eventTimelines: [string, string, number | undefined][] = []

  async announce(): Promise<void> {}

  async offerEventTimeline(name: string, eventType: string, priority?: number): Promise<void> {
    this.eventTimelines.push([name, eventType, priority])
  }

  declareTracks(): Promise<void> {
    this.declared.resolve()
    return this.declaring
  }

  /// The callback the last `addSource` was given.
  onDemandChange: (trackName: string, wanted: boolean) => void = () => {}

  addSource(...args: unknown[]): FakeMediaSource {
    this.onDemandChange = args.at(-1) as (trackName: string, wanted: boolean) => void
    const source = new FakeMediaSource()
    this.sources.push(source)
    return source
  }
}

/// `capturedTracks` lists the tracks `getUserMedia` captured; `workers` every
/// encoder worker started.
export class FakeCaptureEnvironment {
  readonly workers: FakeWorker[] = []
  readonly capturedTracks: FakeTrack[] = []
  readonly broadcast = new FakeMediaBroadcast()
  readonly session = { broadcast: () => this.broadcast } as unknown as MoqSession
  private readonly originals = new Map<string, PropertyDescriptor | undefined>()

  static install(): FakeCaptureEnvironment {
    const environment = new FakeCaptureEnvironment()
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

  encoders(track: FakeTrack): FakeWorker[] {
    return this.workers.filter((worker) => worker.encodes(track))
  }

  private fakeGlobals(): Record<(typeof FAKED_GLOBALS)[number], unknown> {
    const environment = this
    return {
      MediaStream: FakeMediaStream,
      MediaStreamTrack: FakeTrack,
      Worker: class extends FakeWorker {
        constructor() {
          super()
          environment.workers.push(this)
        }
      },
      navigator: {
        mediaDevices: {
          getUserMedia: async (constraints: MediaStreamConstraints) => {
            const tracks = (['video', 'audio'] as const)
              .filter((kind) => constraints[kind] !== undefined)
              .map((kind) => new FakeTrack(kind))
            environment.capturedTracks.push(...tracks)
            return new FakeMediaStream(tracks)
          }
        }
      }
    }
  }
}
