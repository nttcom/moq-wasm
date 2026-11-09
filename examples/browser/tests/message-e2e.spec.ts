import { expect, test, type Locator } from '@playwright/test'
import { arrangeMessageE2ESession, messageE2EConfig, type MessagePageModel } from './message-e2e-arrange'

const TRACK_NAME = 'e2e_track'
const PAYLOAD = 'e2e-hello'

async function send(client: MessagePageModel, button: Locator, label: string): Promise<void> {
  // Act: 対象のリクエストを送る。
  await button.click()
  // Assert: relay が受理したことをステータス表示で確認する。
  await expect(client.sendStatus).toHaveText(`Sent ${label}`)
}

async function arrangeTrackFields(client: MessagePageModel): Promise<void> {
  // Arrange: namespace と track を使うフィールドをすべてテスト用の値に揃える。
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
      // Assert: ローカル検証用の relay URL が初期表示されていることを確認する。
      await expect(client.urlInput).toHaveValue(messageE2EConfig.moqtUrl)

      // Act: relay へ接続する。
      await client.connectButton.click()
      // Assert
      await expect(client.sendStatus).toHaveText('Connected')
      await arrangeTrackFields(client)

      await send(client, client.announceButton, 'PUBLISH_NAMESPACE')
      await send(client, client.watchButton, 'SUBSCRIBE_NAMESPACE')
      // Act: 自分の namespace の track を配信する。
      await client.publishTrackButton.click()
      // Assert
      await expect(client.sendStatus).toHaveText(`Published ${messageE2EConfig.namespace}/${TRACK_NAME}`)

      // relay は SUBSCRIBE を publisher であるこのセッションへ転送し、TrackWriter が応答する。
      await send(client, client.subscribeButton, 'SUBSCRIBE')
      await send(client, client.writeGroupButton, 'OBJECT')
      // Assert: 書いた object が relay を経て自分の subscription に届いたことを確認する。
      await expect(client.received).toContainText(new RegExp(`\\[subscribe\\] \\d+/0: ${PAYLOAD}`))

      await send(client, client.trackStatusButton, 'TRACK_STATUS')
      // Assert: TRACK_STATUS が largest を返すことを確認する。
      await expect(client.received).toContainText(/\[track status\] largest \d+\/\d+/)

      // Arrange: writeGroup が FETCH の範囲をその group に揃えている。
      await send(client, client.fetchButton, 'FETCH')
      // Assert: FETCH が書いた object を返したことを確認する。
      await expect(client.received).toContainText(new RegExp(`\\[fetch\\] \\d+/0: ${PAYLOAD}`))

      // Act: forward を止める。
      await client.subscribeForwardCheckbox.uncheck()
      await send(client, client.setForwardButton, 'SUBSCRIBE_UPDATE')
      await send(client, client.unsubscribeButton, 'UNSUBSCRIBE')
      await send(client, client.withdrawButton, 'PUBLISH_NAMESPACE_DONE')
      await send(client, client.unwatchButton, 'UNSUBSCRIBE_NAMESPACE')
      // SUBSCRIBE_NAMESPACE を最後にもう一度送る。応答が届けば、一連のリクエストのあともセッションが生きている。
      await send(client, client.watchButton, 'SUBSCRIBE_NAMESPACE')

      // Assert: 一連のリクエストでセッションが切断されていないことを確認する。
      await expect(client.logPanel).not.toContainText('connection closed')
    } finally {
      await context.close()
    }
  })
})
