import { expect, test } from '@playwright/test'
import { followNamespaces } from './followNamespaces'
import { FakeTextTrackSession } from '../player/testing/fakeTextTrackSession'

const ROOM = ['app', 'room']
const BOB = ['app', 'room', 'bob']
const ALICE = ['app', 'room', 'alice']

type FakeBroadcast = { namespace: string[]; disposed: boolean; dispose(): Promise<void> }

/// Lets the reader of the watch take what was announced.
function settle(): Promise<void> {
  return new Promise((resolve) => setTimeout(resolve, 0))
}

/// Every callback in the order it was called, as `add app/room/alice`.
async function following(fake: FakeTextTrackSession, filter?: (namespace: string[]) => boolean): Promise<string[]> {
  const heard: string[] = []
  const watch = await fake.session.watchNamespaces(ROOM)
  void followNamespaces(
    fake.session,
    watch,
    new Map<string, FakeBroadcast>(),
    {
      filter,
      play: (namespace) => ({
        namespace,
        disposed: false,
        async dispose() {
          this.disposed = true
        }
      }),
      onAdd: (broadcast) => heard.push(`add ${broadcast.namespace.join('/')}`),
      onRemove: (broadcast) => heard.push(`remove ${broadcast.namespace.join('/')}`)
    },
    () => false
  )
  return heard
}

test('a namespace the filter excludes is not played', async () => {
  // Arrange
  const fake = new FakeTextTrackSession()
  const heard = await following(fake, (namespace) => namespace.at(-1) !== 'bob')

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
  const heard = await following(fake)
  fake.watches[0].announce(ALICE)
  await settle()

  // Act
  fake.watches[0].announce(ALICE)
  await settle()

  // Assert
  expect(heard).toEqual(['add app/room/alice', 'remove app/room/alice', 'add app/room/alice'])
})
