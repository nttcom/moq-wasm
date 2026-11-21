import { expect, test } from '@playwright/test'
import { decodeControl, decodeState, encodeAction, velocityOf } from './teleopWire'

const encode = (value: unknown) => new TextEncoder().encode(JSON.stringify(value))

test('the held keys give a velocity per axis, opposite keys cancelling', () => {
  // Act / Assert
  expect(velocityOf(['ArrowRight', 'KeyW'], 0.5)).toEqual([0.5, 0.5])
  expect(velocityOf(['ArrowLeft', 'KeyD', 'ArrowDown'], 0.5)).toEqual([0, -0.5])
  expect(velocityOf(['KeyQ'], 0.5)).toEqual([0, 0])
})

test("an action is one record of a one-entry chunk in the design's shape", () => {
  // Act
  const payload = encodeAction({ seq: 7, basis: 1_000, dtMs: 50, velocity: [0.5, 0] }, 1_020.4)

  // Assert
  expect(JSON.parse(new TextDecoder().decode(payload))).toEqual([
    { T: 1_020, data: { seq: 7, basis: 1_000, dt_ms: 50, actions: [[0.5, 0]] } }
  ])
})

test('state and control are read from the last record and malformed objects are skipped', () => {
  // Act / Assert
  expect(decodeState(encode([{ T: 5, data: { seq: 1, x: 0.25, y: -0.5 } }]))).toEqual({ t: 5, x: 0.25, y: -0.5 })
  expect(decodeControl(encode([{ T: 5, data: { driver: null, reason: 'stale' } }]))).toEqual({
    driver: null,
    reason: 'stale'
  })
  expect(decodeState(encode([{ T: 5, data: { seq: 1 } }]))).toBeUndefined()
  expect(decodeControl(new TextEncoder().encode('not json'))).toBeUndefined()
})
