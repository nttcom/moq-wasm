import { expect, test } from '@playwright/test'
import type { MoqSession, MoqSubscribeOptions } from '../../core/session'
import type { DeliveryObserver } from './deliveryObserver'
import type { MediaKind } from './trackContext'
import { TrackSubscriptions } from './trackSubscriptions'

function recordingSubscriptions(): { subscriptions: TrackSubscriptions; priorities: Map<string, number | undefined> } {
  const priorities = new Map<string, number | undefined>()
  const session = {
    isOpen: true,
    subscribe: async (_namespace: string[], track: string, options: MoqSubscribeOptions) => {
      priorities.set(track, options.priority)
      return { done: new Promise(() => {}), groups: async function* () {} }
    }
  } as unknown as MoqSession
  const observer = { label: () => {}, forget: () => {} } as unknown as DeliveryObserver
  const context = { session, namespace: ['room'], observer, log: () => {} }
  return { subscriptions: new TrackSubscriptions(context, () => {}), priorities }
}

function track(name: string) {
  return { name, label: name }
}

test('audio is subscribed with a lower subscriber priority value than video, so the relay sends it first', async () => {
  // Arrange
  const { subscriptions, priorities } = recordingSubscriptions()
  const subscribe = (kind: MediaKind, name: string) => subscriptions.subscribe(kind, name, track(name), () => {})

  // Act
  await subscribe('video', 'video-720p')
  await subscribe('audio', 'audio')

  // Assert
  expect(priorities).toEqual(
    new Map([
      ['video-720p', 128],
      ['audio', 0]
    ])
  )
})
