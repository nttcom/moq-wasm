import { expect, test } from '@playwright/test'
import {
  arrangeMediaE2ESession,
  mediaE2EConfig,
  type PublisherPageModel,
  type SubscriberPageModel
} from './media-e2e-arrange'

async function expectInitialConfiguration(
  publisher: PublisherPageModel,
  subscriber: SubscriberPageModel
): Promise<void> {
  // Assert
  await expect(publisher.urlInput).toHaveValue(mediaE2EConfig.moqtUrl)
  await expect(subscriber.urlInput).toHaveValue(mediaE2EConfig.moqtUrl)
  await expect(publisher.namespaceInput).toHaveValue(mediaE2EConfig.namespace)
  await expect(subscriber.namespaceInput).toHaveValue(mediaE2EConfig.namespace)
}

async function startPublisherBroadcast(publisher: PublisherPageModel): Promise<void> {
  // Act
  await publisher.connectButton.click()
  // Assert: WebTransport by default, WebSocket through the TCP proxy.
  await expect(publisher.connectionStatus).toContainText('Connected:')
  await expect.poll(() => publisher.sessionTransports).toEqual([mediaE2EConfig.expectedTransport])

  // Act
  await publisher.getUserMediaButton.click()
  // Assert
  await expect(publisher.captureStatus).toContainText('Media ready')

  // Act
  await publisher.sendButton.click()
  // Assert
  await expect(publisher.sendStatus).toContainText(`Publishing started: ${mediaE2EConfig.namespace}`)
}

async function subscribeForPlayback(subscriber: SubscriberPageModel): Promise<void> {
  // Act
  await subscriber.connectButton.click()
  // Assert
  await expect(subscriber.connectionStatus).toContainText('Connected:')
  await expect.poll(() => subscriber.sessionTransports).toEqual([mediaE2EConfig.expectedTransport])

  // Act
  await subscriber.catalogSubscribeButton.click()
  // Assert
  await expect(subscriber.catalogStatus).toContainText('Catalog loaded: video=2')

  // Act
  await subscriber.videoTrackSelect.selectOption('camera_low')
  await subscriber.trackSubscribeButton.click()
}

async function expectVideoMuteToLeaveAndRejoinTheCatalog(
  publisher: PublisherPageModel,
  subscriber: SubscriberPageModel
): Promise<void> {
  // Act
  await publisher.muteVideoButton.click()
  // Assert
  await expect(subscriber.catalogStatus).toContainText('video=0')

  // Act
  await publisher.muteVideoButton.click()
  // Assert: wait 2 s to ensure no send error overwrites the unmuted status.
  await expect(subscriber.catalogStatus).toContainText('video=2')
  await publisher.sendStatus.page().waitForTimeout(2_000)
  await expect(publisher.sendStatus).toHaveText('Video unmuted')
}

async function expectScreenShareToJoinAndLeaveTheCatalog(
  publisher: PublisherPageModel,
  subscriber: SubscriberPageModel
): Promise<void> {
  // Act
  await publisher.shareScreenButton.click()
  // Assert
  await expect(subscriber.catalogStatus).toContainText('video=3')

  // Act
  await publisher.shareScreenButton.click()
  // Assert
  await expect(subscriber.catalogStatus).toContainText('video=2')
}

async function expectSubscriberPlayback(subscriber: SubscriberPageModel): Promise<void> {
  // Assert
  await expect(subscriber.receiveStatus).toHaveText(/Received video objects: [1-9]/)
  await expect(subscriber.playbackStatus).toContainText('Playing')
  await expect.poll(async () => subscriber.video.evaluate((video) => video.readyState)).toBeGreaterThanOrEqual(2)
  await expect.poll(async () => subscriber.video.evaluate((video) => video.currentTime)).toBeGreaterThan(0)
  await expect.poll(async () => subscriber.video.evaluate((video) => video.paused)).toBe(false)

  const playback = await subscriber.video.evaluate((video) => ({
    currentTime: video.currentTime,
    readyState: video.readyState
  }))
  expect(playback.readyState).toBeGreaterThanOrEqual(2)
  expect(playback.currentTime).toBeGreaterThan(0)
}

test('publisher streams video to subscriber over local MoQT server', async ({ browser }) => {
  // Arrange
  const session = await arrangeMediaE2ESession(browser)

  try {
    // Assert
    await expectInitialConfiguration(session.publisher, session.subscriber)

    // Act / Assert
    await startPublisherBroadcast(session.publisher)
    await subscribeForPlayback(session.subscriber)

    // Assert
    await expectSubscriberPlayback(session.subscriber)

    // Assert: only the subscribed rendition encodes; unsubscribed camera stays at 0 kbps.
    await expect(session.publisher.encodeStatus).toHaveText(/camera 0 kbps \/ low [1-9]\d* kbps/)

    // Act / Assert
    await expectVideoMuteToLeaveAndRejoinTheCatalog(session.publisher, session.subscriber)
    await expectScreenShareToJoinAndLeaveTheCatalog(session.publisher, session.subscriber)
  } finally {
    await session.context.close()
  }
})
