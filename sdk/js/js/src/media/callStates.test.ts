import { expect, test } from '@playwright/test'
import { CallStates, type CallStateUpdate, parseState } from './callStates'
import { FakeTextTrackSession } from './player/testing/fakeTextTrackSession'

const CALL = ['app', 'room', 'phone', 'abc']

/// Lets the reader of a subscription take what was delivered.
function settle(): Promise<void> {
  return new Promise((resolve) => setTimeout(resolve, 0))
}

function following(fake: FakeTextTrackSession): { updates: CallStateUpdate[]; states: CallStates; run: Promise<void> } {
  const updates: CallStateUpdate[] = []
  const states = new CallStates(fake.session, CALL, (update) => updates.push(update))
  return { updates, states, run: states.run() }
}

test('the states are read from the current group until the call ends', async () => {
  // Arrange
  const fake = new FakeTextTrackSession()
  const { updates, run } = following(fake)
  await settle()
  const [subscription] = fake.open('call')

  // Act
  subscription.deliver(3n, '{"state":"ringing"}')
  subscription.deliver(4n, '{"state":"ended","reason":"486"}')
  await run

  // Assert
  expect(updates).toEqual([
    { state: 'ringing', reason: undefined },
    { state: 'ended', reason: '486' }
  ])
})

test('the track is subscribed again until the gateway publishes it', async () => {
  // Arrange
  const fake = new FakeTextTrackSession()
  fake.absent = true
  const { updates, states } = following(fake)
  await settle()

  // Act
  fake.absent = false
  await new Promise((resolve) => setTimeout(resolve, 600))
  fake.open('call')[0].deliver(0n, '{"state":"dialing"}')
  await settle()

  // Assert
  expect(updates).toEqual([{ state: 'dialing', reason: undefined }])
  await states.close()
})

test('a track that ends without ended ends the call with gateway', async () => {
  // Arrange
  const fake = new FakeTextTrackSession()
  const { updates, run } = following(fake)
  await settle()
  const [subscription] = fake.open('call')
  subscription.deliver(0n, '{"state":"answered"}')
  await settle()

  // Act
  subscription.end({ code: 0n, reason: '' })
  await run

  // Assert
  expect(updates).toEqual([
    { state: 'answered', reason: undefined },
    { state: 'ended', reason: 'gateway' }
  ])
})

test('an object that is not a state is skipped', async () => {
  // Arrange
  const fake = new FakeTextTrackSession()
  const { updates, states } = following(fake)
  await settle()
  const [subscription] = fake.open('call')

  // Act
  subscription.deliver(0n, 'not json')
  subscription.deliver(1n, '{"state":"ringing"}')
  await settle()

  // Assert
  expect(updates).toEqual([{ state: 'ringing', reason: undefined }])
  await states.close()
})

test('a state is a known name with a reason only when it is a string', () => {
  // Act / Assert
  expect(parseState('{"state":"ended","reason":"hangup"}')).toEqual({ state: 'ended', reason: 'hangup' })
  expect(parseState('{"state":"ended","reason":486}')).toEqual({ state: 'ended', reason: undefined })
  expect(parseState('{"state":"held"}')).toBeUndefined()
  expect(parseState('[]')).toBeUndefined()
  expect(parseState('null')).toBeUndefined()
})
