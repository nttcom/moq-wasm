import { expect, test } from '@playwright/test'
import { AddedTextTrack } from './addedTextTrack'
import { FakeMediaEnvironment } from './testing/fakeMediaEnvironment'
import { FakeSession, receivedObject } from './testing/fakeSession'

const MICROS_PER_MILLI = 1_000
const TEXT = '\u0001'

let environment: FakeMediaEnvironment

test.beforeEach(() => {
  environment = FakeMediaEnvironment.install({ outputLatencySeconds: 0 })
})

test.afterEach(() => {
  environment.uninstall()
})

test('added text is delivered when its capture time is due, late text at once with its lateness, and none after close', async () => {
  // Arrange: a capture time is due that many milliseconds on the page clock
  const startMs = environment.nowMs
  const at = (offsetMs: number) => (startMs + offsetMs) * MICROS_PER_MILLI
  const session = new FakeSession(undefined, [
    receivedObject(1n, 0n, at(-30)),
    receivedObject(1n, 1n, at(50)),
    receivedObject(1n, 2n, at(500))
  ])
  const playout = { dueAt: (captureMicros: number) => captureMicros / MICROS_PER_MILLI }
  const delivered: [string, number, number][] = []
  const added = await AddedTextTrack.open(
    session.context(),
    playout,
    'text_en',
    { onText: (text, lateMs) => delivered.push([text, lateMs, environment.nowMs - startMs]) },
    () => {}
  )
  await session.drained

  // Act
  environment.advanceTo(startMs + 100)
  await added.close()
  environment.advanceTo(startMs + 1_000)

  // Assert
  expect(session.calls).toEqual([{ method: 'subscribeCurrentGroup', track: 'text_en' }])
  expect(delivered).toEqual([
    [TEXT, 30, 0],
    [TEXT, 0, 50]
  ])
  expect(session.updates).toEqual(['close text_en'])
})
