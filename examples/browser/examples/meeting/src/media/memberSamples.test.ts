import { expect, test } from '@playwright/test'
import type { LiveStats } from 'moq-wasm/media'
import { memberSamples } from './memberSamples'
import type { RemoteMember } from '../types/member'

test('each video a member plays is one sample named by its namespace and source', () => {
  // Arrange
  const member = { id: 'alice', trackNamespace: ['anon', 'meeting', 'room', 'alice'] } as RemoteMember
  const camera = { bufferMs: 80 } as LiveStats
  const subscriber = { stats: () => ({ camera, screenshare: undefined }) }

  // Act
  const samples = memberSamples(subscriber, [member])

  // Assert: the screen share that is not playing has no sample
  expect(samples).toEqual([{ trackNamespace: 'anon/meeting/room/alice', mediaSource: 'camera', values: camera }])
})
