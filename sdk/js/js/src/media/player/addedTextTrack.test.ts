import { expect, test } from '@playwright/test'
import { AddedTextTrack } from './addedTextTrack'
import { FakeMediaEnvironment } from './testing/fakeMediaEnvironment'
import { FakeSession, receivedObject } from './testing/fakeSession'
import { collectReportedErrors } from '../internal/testing/reportedErrors'

const MICROS_PER_MILLI = 1_000
const TEXT = '\u0001'

const reported = collectReportedErrors()
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
  expect(session.calls).toEqual([{ method: 'subscribeCurrentGroup', track: 'text_en', options: { priority: 16 } }])
  expect(delivered).toEqual([
    [TEXT, 30, 0],
    [TEXT, 0, 50]
  ])
  expect(session.updates).toEqual(['close text_en'])
})

test('an onText that throws is reported and the later texts are still delivered', async () => {
  // Arrange
  const startMs = environment.nowMs
  const at = (offsetMs: number) => (startMs + offsetMs) * MICROS_PER_MILLI
  const session = new FakeSession(undefined, [receivedObject(1n, 0n, at(10)), receivedObject(1n, 1n, at(20))])
  const playout = { dueAt: (captureMicros: number) => captureMicros / MICROS_PER_MILLI }
  const error = new Error('onText bug')
  const delivered: number[] = []
  await AddedTextTrack.open(
    session.context(),
    playout,
    'text_en',
    {
      onText: () => {
        delivered.push(environment.nowMs - startMs)
        throw error
      }
    },
    () => {}
  )
  await session.drained

  // Act
  environment.advanceTo(startMs + 100)

  // Assert
  expect(reported).toEqual([error, error])
  expect(delivered).toEqual([10, 20])
})
