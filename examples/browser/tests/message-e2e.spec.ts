import { expect, test, type Locator } from '@playwright/test'
import { arrangeMessageE2ESession, messageE2EConfig, type MessagePageModel } from './message-e2e-arrange'

async function connectAndSetup(client: MessagePageModel): Promise<void> {
  // Assert: ローカル検証用の relay URL が初期表示されていることを確認する。
  await expect(client.urlInput).toHaveValue(messageE2EConfig.moqtUrl)

  // Act: relay へ接続する。
  await client.connectButton.click()

  // Act: CLIENT_SETUP を送信してセッションを確立する。
  await client.setupButton.click()
  // Assert: SERVER_SETUP がログに現れ、以降の制御メッセージを送れる状態になったことを確認する。
  await expect(client.logPanel).toContainText('serverSetup')
}

async function send(client: MessagePageModel, button: Locator, label: string): Promise<void> {
  // Act: 対象の制御メッセージを送信する。
  await button.click()
  // Assert: encode と送信が成功したことをステータス表示で確認する。
  await expect(client.sendStatus).toHaveText(`Sent ${label}`)
}

test.describe('draft-14 control messages', () => {
  test('relay accepts every newly implemented control message without dropping the session', async ({ browser }) => {
    const { context, client } = await arrangeMessageE2ESession(browser)

    try {
      await connectAndSetup(client)

      // Arrange: この検証で使う namespace をテスト用の値に揃える。
      await client.publishNamespaceCancelInput.fill(messageE2EConfig.namespace)
      await client.trackStatusNamespaceInput.fill(messageE2EConfig.namespace)
      // Arrange: GOAWAY の New Session URI は空にする。draft-14 §9.4 では
      // サーバーが非空の URI を受け取ると PROTOCOL_VIOLATION で切断する。
      await client.goAwayUriInput.fill('')

      await send(client, client.goAwayButton, 'GOAWAY')
      await send(client, client.maxRequestIdButton, 'MAX_REQUEST_ID')
      await send(client, client.requestsBlockedButton, 'REQUESTS_BLOCKED')
      await send(client, client.fetchCancelButton, 'FETCH_CANCEL')
      await send(client, client.subscribeUpdateButton, 'SUBSCRIBE_UPDATE')
      await send(client, client.publishDoneButton, 'PUBLISH_DONE')
      await send(client, client.publishNamespaceCancelButton, 'PUBLISH_NAMESPACE_CANCEL')
      // TRACK_STATUS は最後に送る。relay からの応答が届くことで、直前の
      // 7 メッセージを受信したあともセッションが生きていることが分かる。
      await send(client, client.trackStatusButton, 'TRACK_STATUS')

      // Assert: relay が TRACK_STATUS を受信し、未実装ハンドラの応答として
      // TRACK_STATUS_ERROR を返したことを確認する。
      await expect(client.logPanel).toContainText('TrackStatusError')
      // Assert: 一連の送信でセッションが切断されていないことを確認する。
      await expect(client.logPanel).not.toContainText('connection closed')
    } finally {
      await context.close()
    }
  })
})
