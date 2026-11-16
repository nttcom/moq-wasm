import { expect, test } from '@playwright/test'
import { MEDIA_CMAF_PUBLISHER_PATH, MEDIA_CMAF_SUBSCRIBER_PATH } from '../playwright.helpers'
import { arrangeMediaE2ESession, mediaE2EConfig } from './media-e2e-arrange'

test('a subscriber plays the CMAF video of a broadcast published with cmaf: true', async ({ browser }) => {
  // Arrange: the CMAF publisher page publishes the camera; the subscriber page opens on the same URL and namespace
  const session = await arrangeMediaE2ESession(browser, {
    publisher: MEDIA_CMAF_PUBLISHER_PATH,
    subscriber: MEDIA_CMAF_SUBSCRIBER_PATH
  })
  const { publisher, subscriber } = session
  const shownVideo = subscriber.playbackStatus.page().getByTestId('subscriber-stage').locator('video:not([hidden])')

  try {
    await publisher.connectButton.click()
    await expect(publisher.connectionStatus).toContainText('Connected:')
    await publisher.getUserMediaButton.click()
    await expect(publisher.captureStatus).toContainText('Media ready')
    await publisher.sendButton.click()
    await expect(publisher.sendStatus).toContainText(`Publishing started: ${mediaE2EConfig.namespace}`)

    // Act: only the CMAF sibling is subscribed, so the video is encoded for it alone
    await subscriber.connectButton.click()
    await expect(subscriber.connectionStatus).toContainText('Connected:')
    await subscriber.trackSubscribeButton.click()

    // Assert: the MSE video on screen advances
    await expect(subscriber.playbackStatus).toContainText('Playing')
    await expect.poll(() => shownVideo.evaluate((video: HTMLVideoElement) => video.currentTime)).toBeGreaterThan(0)
    await expect.poll(() => shownVideo.evaluate((video: HTMLVideoElement) => video.paused)).toBe(false)
  } finally {
    await session.context.close()
  }
})
