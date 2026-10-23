import { expect, test } from '@playwright/test'
import type { SubgroupSender } from '@moqt/moqtClient'
import { audioEndOfGroupTiming, sendAudioEndOfGroupViaMoqt } from './audioTransport'
import { OBJECT_STATUS_END_OF_GROUP } from './objectStatus'
import { MediaTransportState } from './transportState'

type SentObject = { alias: bigint; groupId: bigint; objectId: bigint; status: number | undefined }

function recordingSender(sent: SentObject[]): SubgroupSender {
  return {
    sendSubgroupHeader: async () => {},
    sendSubgroupObject: async (alias, groupId, _subgroupId, objectId, status) => {
      sent.push({ alias, groupId, objectId, status })
    }
  }
}

test('the group ends only on aliases whose stream is open in it', async () => {
  // Arrange
  const transportState = new MediaTransportState()
  transportState.audio.markHeaderSent(1n, 0)
  transportState.audio.nextObjectId = 5n
  const sent: SentObject[] = []

  // Act
  await sendAudioEndOfGroupViaMoqt([1n, 2n], recordingSender(sent), transportState)

  // Assert
  expect(sent).toEqual([
    { alias: 1n, groupId: transportState.audio.groupId, objectId: 5n, status: OBJECT_STATUS_END_OF_GROUP }
  ])
})

test('the time an EndOfGroup takes to send and close is recorded', async () => {
  // Arrange: a sender that takes 5 ms to accept the EndOfGroup
  const transportState = new MediaTransportState()
  transportState.audio.markHeaderSent(1n, 0)
  const sender: SubgroupSender = {
    sendSubgroupHeader: async () => {},
    sendSubgroupObject: () => new Promise((resolve) => setTimeout(resolve, 5))
  }
  const recordedBefore = audioEndOfGroupTiming.durationsMs.length

  // Act
  await sendAudioEndOfGroupViaMoqt([1n], sender, transportState)

  // Assert
  expect(audioEndOfGroupTiming.durationsMs.length).toBe(recordedBefore + 1)
  expect(audioEndOfGroupTiming.durationsMs.at(-1)).toBeGreaterThanOrEqual(4)
})
