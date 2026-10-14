import { expect, test } from '@playwright/test'
import { SubscriptionStateManager, type SubgroupObjectMessageWithLoc } from './subscriptionStateManager'

const TRACK_ALIAS = 3n

function subgroupObject(): SubgroupObjectMessageWithLoc {
  return {} as SubgroupObjectMessageWithLoc
}

test('objects that arrive before the handler reach it once it is set', () => {
  // Arrange
  const state = new SubscriptionStateManager()
  const received: bigint[] = []
  state.bufferSubgroupObject(TRACK_ALIAS, 7n, subgroupObject())

  // Act
  state.setSubgroupObjectHandler(TRACK_ALIAS, (groupId) => received.push(groupId))

  // Assert
  expect(received).toEqual([7n])
})

test('objects that arrive after the publisher ended the subscription are dropped', () => {
  // Arrange
  const state = new SubscriptionStateManager()
  const received: bigint[] = []
  state.setSubgroupObjectHandler(TRACK_ALIAS, (groupId) => received.push(groupId))
  state.endSubscription(TRACK_ALIAS)

  // Act
  state.bufferSubgroupObject(TRACK_ALIAS, 7n, subgroupObject())
  state.setSubgroupObjectHandler(TRACK_ALIAS, (groupId) => received.push(groupId))

  // Assert
  expect(received).toEqual([])
})
