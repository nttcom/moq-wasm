import { expect, test } from '@playwright/test'
import { FakeMediaEnvironment } from '../player/testing/fakeMediaEnvironment'
import { GroupOrderGate } from './groupOrderGate'
import type { SubgroupObjectWithLoc } from './jitterBufferTypes'
import { buildLocHeader } from './loc'

const CAPTURE_MICROS = 1_000_000_000

let environment: FakeMediaEnvironment

test.beforeEach(() => {
  environment = FakeMediaEnvironment.install({ outputLatencySeconds: 0 })
})

test.afterEach(() => {
  environment.uninstall()
})

function audioObject(objectId: number, captureMicros: number): SubgroupObjectWithLoc {
  return {
    subgroupId: 0n,
    objectId: BigInt(objectId),
    objectPayloadLength: 1,
    objectPayload: Uint8Array.of(0),
    objectStatus: undefined,
    locHeader: buildLocHeader({ captureTimestampMicros: captureMicros })
  }
}

function recordingGate(): { gate: GroupOrderGate; released: string[] } {
  const released: string[] = []
  const gate = new GroupOrderGate((groupId, object) => released.push(`${groupId}/${object.objectId}`))
  return { gate, released }
}

test('without a timeline the head of a later group is released 100 ms after it arrived', () => {
  // Arrange
  const { gate, released } = recordingGate()
  gate.push(1n, audioObject(0, CAPTURE_MICROS))
  gate.push(2n, audioObject(0, CAPTURE_MICROS + 40_000))

  // Act
  environment.advanceTo(environment.nowMs + 99)
  const releasedBeforeHold = [...released]
  environment.advanceTo(environment.nowMs + 1)

  // Assert
  expect(releasedBeforeHold).toEqual(['1/0'])
  expect(released).toEqual(['1/0', '2/0'])
})

test('with a timeline the tail of a group arriving 300 ms late is released before the next group', () => {
  // Arrange: the head of group 2 has to leave the gate 500 ms from now
  const { gate, released } = recordingGate()
  gate.setTimeline({ captureMicros: CAPTURE_MICROS + 40_000, releaseAtMs: environment.nowMs + 500 })
  gate.push(1n, audioObject(0, CAPTURE_MICROS))
  gate.push(2n, audioObject(0, CAPTURE_MICROS + 40_000))

  // Act
  environment.advanceTo(environment.nowMs + 300)
  gate.push(1n, audioObject(1, CAPTURE_MICROS + 20_000))
  environment.advanceTo(environment.nowMs + 200)

  // Assert
  expect(released).toEqual(['1/0', '1/1', '2/0'])
})
