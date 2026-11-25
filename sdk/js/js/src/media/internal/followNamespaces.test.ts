import { expect, test } from '@playwright/test'
import { followNamespaces } from './followNamespaces'
import type { LogRecord } from '../../core/init'
import { setSdkLogSink } from '../../core/log'
import { FakeTextTrackSession } from '../player/testing/fakeTextTrackSession'

const ROOM = ['app', 'room']
const BOB = ['app', 'room', 'bob']
const ALICE = ['app', 'room', 'alice']

type FakeBroadcast = { namespace: string[]; disposed: boolean; dispose(): Promise<void> }

type Behaviour = {
  filter?: (namespace: string[]) => boolean
  play?: (namespace: string[]) => void
  onAdd?: (namespace: string[]) => void
  onRemove?: (namespace: string[]) => void
  dispose?: (namespace: string[]) => Promise<void>
}

let reported: unknown[] = []
let originalReportError: PropertyDescriptor | undefined

// `reportError` is a browser global the unit tests' Node may not have.
test.beforeEach(() => {
  reported = []
  originalReportError = Object.getOwnPropertyDescriptor(globalThis, 'reportError')
  globalThis.reportError = (error: unknown) => reported.push(error)
})

test.afterEach(() => {
  if (originalReportError) {
    Object.defineProperty(globalThis, 'reportError', originalReportError)
  } else {
    Reflect.deleteProperty(globalThis, 'reportError')
  }
})

/// Lets the reader of the watch take what was announced.
function settle(): Promise<void> {
  return new Promise((resolve) => setTimeout(resolve, 0))
}

type Following = {
  /// Every `onAdd` and `onRemove` in the order it was called, as `add app/room/alice`.
  heard: string[]
  /// Every broadcast `play` made.
  broadcasts: FakeBroadcast[]
  /// The broadcasts played now.
  played: Map<string, FakeBroadcast>
  /// As `BroadcastWatch.close`.
  close(): Promise<void>
}

async function following(fake: FakeTextTrackSession, behaviour: Behaviour = {}): Promise<Following> {
  const heard: string[] = []
  const broadcasts: FakeBroadcast[] = []
  const played = new Map<string, FakeBroadcast>()
  let closed = false
  const watch = await fake.session.watchNamespaces(ROOM)
  void followNamespaces(
    fake.session,
    watch,
    played,
    {
      filter: (namespace) => behaviour.filter?.(namespace) ?? true,
      play: (namespace) => {
        behaviour.play?.(namespace)
        const broadcast = {
          namespace,
          disposed: false,
          async dispose() {
            this.disposed = true
            await behaviour.dispose?.(namespace)
          }
        }
        broadcasts.push(broadcast)
        return broadcast
      },
      onAdd: (broadcast) => {
        heard.push(`add ${broadcast.namespace.join('/')}`)
        behaviour.onAdd?.(broadcast.namespace)
      },
      onRemove: (broadcast) => {
        heard.push(`remove ${broadcast.namespace.join('/')}`)
        behaviour.onRemove?.(broadcast.namespace)
      }
    },
    () => closed
  )
  const close = async () => {
    closed = true
    await watch.close()
    const all = [...played.values()]
    played.clear()
    await Promise.all(all.map((broadcast) => broadcast.dispose()))
  }
  return { heard, broadcasts, played, close }
}

function throwsForBob(error: Error): (namespace: string[]) => undefined {
  return (namespace) => {
    if (namespace.at(-1) === 'bob') {
      throw error
    }
    return undefined
  }
}

test('a namespace the filter excludes is not played', async () => {
  // Arrange
  const fake = new FakeTextTrackSession()
  const { heard } = await following(fake, { filter: (namespace) => namespace.at(-1) !== 'bob' })

  // Act
  fake.watches[0].announce(BOB)
  fake.watches[0].announce(ALICE)
  fake.watches[0].withdraw(BOB)
  await settle()

  // Assert
  expect(heard).toEqual(['add app/room/alice'])
})

test('a namespace announced again is removed and added anew', async () => {
  // Arrange
  const fake = new FakeTextTrackSession()
  const { heard } = await following(fake)
  fake.watches[0].announce(ALICE)
  await settle()

  // Act
  fake.watches[0].announce(ALICE)
  await settle()

  // Assert
  expect(heard).toEqual(['add app/room/alice', 'remove app/room/alice', 'add app/room/alice'])
})

test('a filter that throws is reported, its namespace is not played and later events are still delivered', async () => {
  // Arrange
  const fake = new FakeTextTrackSession()
  const error = new Error('filter bug')
  const filter = throwsForBob(error)
  const { heard } = await following(fake, { filter: (namespace) => filter(namespace) ?? true })

  // Act
  fake.watches[0].announce(BOB)
  fake.watches[0].announce(ALICE)
  fake.watches[0].withdraw(ALICE)
  await settle()

  // Assert
  expect(reported).toEqual([error])
  expect(heard).toEqual(['add app/room/alice', 'remove app/room/alice'])
})

test('an onAdd that throws is reported and later events are still delivered', async () => {
  // Arrange
  const fake = new FakeTextTrackSession()
  const error = new Error('onAdd bug')
  const { heard } = await following(fake, { onAdd: throwsForBob(error) })

  // Act
  fake.watches[0].announce(BOB)
  fake.watches[0].announce(ALICE)
  fake.watches[0].withdraw(ALICE)
  await settle()

  // Assert
  expect(reported).toEqual([error])
  expect(heard).toEqual(['add app/room/bob', 'add app/room/alice', 'remove app/room/alice'])
})

test('a broadcast whose onAdd threw is still disposed of and removed when its namespace is withdrawn', async () => {
  // Arrange
  const fake = new FakeTextTrackSession()
  const { heard, broadcasts } = await following(fake, { onAdd: throwsForBob(new Error('onAdd bug')) })
  fake.watches[0].announce(BOB)
  await settle()

  // Act
  fake.watches[0].withdraw(BOB)
  await settle()

  // Assert
  expect(broadcasts.map((broadcast) => broadcast.disposed)).toEqual([true])
  expect(heard).toEqual(['add app/room/bob', 'remove app/room/bob'])
})

test('an onRemove that throws is reported, the removal stands and later events are still delivered', async () => {
  // Arrange
  const fake = new FakeTextTrackSession()
  const error = new Error('onRemove bug')
  const { heard } = await following(fake, { onRemove: throwsForBob(error) })
  fake.watches[0].announce(BOB)
  await settle()

  // Act
  fake.watches[0].withdraw(BOB)
  fake.watches[0].announce(BOB)
  fake.watches[0].announce(ALICE)
  fake.watches[0].withdraw(ALICE)
  await settle()

  // Assert: bob, announced again, is added without a second removal
  expect(reported).toEqual([error])
  expect(heard).toEqual([
    'add app/room/bob',
    'remove app/room/bob',
    'add app/room/bob',
    'add app/room/alice',
    'remove app/room/alice'
  ])
})

test('a played namespace is removed when it is withdrawn although the filter now excludes it', async () => {
  // Arrange
  const fake = new FakeTextTrackSession()
  let accepting = true
  let filtered = 0
  const { heard, broadcasts } = await following(fake, {
    filter: () => {
      filtered += 1
      return accepting
    }
  })
  fake.watches[0].announce(ALICE)
  await settle()
  accepting = false

  // Act
  fake.watches[0].withdraw(ALICE)
  await settle()

  // Assert: the filter was asked about the announcement only
  expect(filtered).toBe(1)
  expect(broadcasts.map((broadcast) => broadcast.disposed)).toEqual([true])
  expect(heard).toEqual(['add app/room/alice', 'remove app/room/alice'])
})

test('a dispose that rejects is logged, still removes the broadcast and the watch goes on', async () => {
  // Arrange
  const fake = new FakeTextTrackSession()
  const logged: LogRecord[] = []
  setSdkLogSink((record) => logged.push(record), 'error')
  const { heard } = await following(fake, {
    dispose: async (namespace) => {
      if (namespace.at(-1) === 'bob') {
        throw new Error('dispose failed')
      }
    }
  })
  fake.watches[0].announce(BOB)
  await settle()

  // Act
  fake.watches[0].withdraw(BOB)
  fake.watches[0].announce(ALICE)
  await settle()

  // Assert: the SDK's own failure is logged, not reported as the page's
  expect(logged.map(({ message, fields }) => [message, fields.track_namespace, fields.error])).toEqual([
    ['disposing of a broadcast failed', 'app/room/bob', 'dispose failed']
  ])
  expect(reported).toEqual([])
  expect(heard).toEqual(['add app/room/bob', 'remove app/room/bob', 'add app/room/alice'])
})

test('a broadcast that fails to start is logged and a later announcement is still played', async () => {
  // Arrange
  const fake = new FakeTextTrackSession()
  const logged: LogRecord[] = []
  setSdkLogSink((record) => logged.push(record), 'error')
  const { heard } = await following(fake, { play: throwsForBob(new Error('play failed')) })

  // Act
  fake.watches[0].announce(BOB)
  fake.watches[0].announce(ALICE)
  await settle()

  // Assert
  expect(logged.map(({ message, fields }) => [message, fields.track_namespace, fields.error])).toEqual([
    ['playing a broadcast failed', 'app/room/bob', 'play failed']
  ])
  expect(reported).toEqual([])
  expect(heard).toEqual(['add app/room/alice'])
})

test('a close during the disposal of a namespace announced again plays nothing more', async () => {
  // Arrange
  const fake = new FakeTextTrackSession()
  let disposed = () => {}
  let pending = false
  const watching = await following(fake, {
    dispose: async () => {
      if (pending) {
        return
      }
      pending = true
      await new Promise<void>((resolve) => {
        disposed = resolve
      })
    }
  })
  fake.watches[0].announce(ALICE)
  await settle()
  fake.watches[0].announce(ALICE)
  await settle()

  // Act
  const closing = watching.close()
  disposed()
  await closing
  await settle()

  // Assert: neither a second onAdd nor, after close, an onRemove
  expect(watching.heard).toEqual(['add app/room/alice'])
  expect(watching.played.size).toBe(0)
  expect(watching.broadcasts.map((broadcast) => broadcast.disposed)).toEqual([true])
})
