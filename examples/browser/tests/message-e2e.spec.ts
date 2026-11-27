import { expect, test, type Locator } from '@playwright/test'
import { arrangeMessageE2ESession, messageE2EConfig, type MessagePageModel } from './message-e2e-arrange'

const TRACK_NAME = 'e2e_track'
const PAYLOAD = 'e2e-hello'

async function send(client: MessagePageModel, button: Locator, label: string): Promise<void> {
  await button.click()
  await expect(client.sendStatus).toHaveText(`Sent ${label}`)
}

async function arrangeTrackFields(client: MessagePageModel): Promise<void> {
  const namespace = messageE2EConfig.namespace
  for (const input of [
    client.announceNamespaceInput,
    client.watchPrefixInput,
    client.publishNamespaceInput,
    client.subscribeNamespaceInput,
    client.fetchNamespaceInput,
    client.trackStatusNamespaceInput
  ]) {
    await input.fill(namespace)
  }
  for (const input of [
    client.publishTrackInput,
    client.subscribeTrackInput,
    client.fetchTrackNameInput,
    client.trackStatusTrackInput
  ]) {
    await input.fill(TRACK_NAME)
  }
  await client.writePayloadInput.fill(PAYLOAD)
}

test.describe('MoqSession requests through the relay', () => {
  test('every request round-trips through the relay without dropping the session', async ({ browser }) => {
    const { context, client } = await arrangeMessageE2ESession(browser)

    try {
      // Assert
      await expect(client.urlInput).toHaveValue(messageE2EConfig.moqtUrl)

      // Act
      await client.connectButton.click()
      // Assert
      await expect(client.sendStatus).toHaveText('Connected')
      await arrangeTrackFields(client)

      await send(client, client.announceButton, 'PUBLISH_NAMESPACE')
      await send(client, client.watchButton, 'SUBSCRIBE_NAMESPACE')
      // Act
      await client.publishTrackButton.click()
      // Assert
      await expect(client.sendStatus).toHaveText(`Published ${messageE2EConfig.namespace}/${TRACK_NAME}`)

      // The relay forwards SUBSCRIBE back to this session as publisher, and TrackWriter answers.
      await send(client, client.subscribeButton, 'SUBSCRIBE')
      await send(client, client.writeGroupButton, 'OBJECT')
      // Assert
      await expect(client.received).toContainText(new RegExp(`\\[subscribe\\] \\d+/0: ${PAYLOAD}`))

      await send(client, client.trackStatusButton, 'TRACK_STATUS')
      // Assert
      await expect(client.received).toContainText(/\[track status\] largest \d+\/\d+/)

      // Arrange: writeGroup aligns the FETCH range with that group.
      await send(client, client.fetchButton, 'FETCH')
      // Assert
      await expect(client.received).toContainText(new RegExp(`\\[fetch\\] \\d+/0: ${PAYLOAD}`))

      // Act
      await client.subscribeForwardCheckbox.uncheck()
      await send(client, client.setForwardButton, 'SUBSCRIBE_UPDATE')
      await send(client, client.unsubscribeButton, 'UNSUBSCRIBE')
      await send(client, client.withdrawButton, 'PUBLISH_NAMESPACE_DONE')
      await send(client, client.unwatchButton, 'UNSUBSCRIBE_NAMESPACE')
      // Send SUBSCRIBE_NAMESPACE once more; receiving a reply confirms the session is still alive.
      await send(client, client.watchButton, 'SUBSCRIBE_NAMESPACE')

      // Assert
      await expect(client.logPanel).not.toContainText('connection closed')
    } finally {
      await context.close()
    }
  })
})
