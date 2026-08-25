import { expect, test, type Locator } from '@playwright/test'
import { arrangeMessageE2ESession, messageE2EConfig, type MessagePageModel } from './message-e2e-arrange'

const TRACK_NAME = 'e2e_track'

async function send(client: MessagePageModel, button: Locator, label: string): Promise<void> {
  // Act: 対象の制御メッセージを送信する。
  await button.click()
  // Assert: encode と送信が成功したことをステータス表示で確認する。
  await expect(client.sendStatus).toHaveText(`Sent ${label}`)
}

async function arrangeNamespaceFields(client: MessagePageModel): Promise<void> {
  // Arrange: namespace を使うフィールドをすべてテスト用の値に揃える。
  const namespace = messageE2EConfig.namespace
  await client.publishNamespaceInput.fill(namespace)
  await client.publishNamespaceDoneInput.fill(namespace)
  await client.subscribeNamespaceInput.fill(namespace)
  await client.unsubscribeNamespaceInput.fill(namespace)
  await client.subscribeNamespaceFieldInput.fill(namespace)
  await client.subscribeTrackNameInput.fill(TRACK_NAME)
  await client.publishMessageNamespaceInput.fill(namespace)
  // FETCH だけは publisher が存在しない namespace を指す。announce 済みの
  // namespace を FETCH すると relay が上流（=このセッション自身）へ転送して
  // FETCH_OK を待ち、後続イベントの処理がブロックされてしまう。
  await client.fetchNamespaceInput.fill(`${namespace}/absent`)
  await client.publishNamespaceCancelInput.fill(namespace)
  await client.trackStatusNamespaceInput.fill(namespace)
  // Arrange: GOAWAY の New Session URI は空にする。draft-14 §9.4 では
  // サーバーが非空の URI を受け取ると PROTOCOL_VIOLATION で切断する。
  await client.goAwayUriInput.fill('')
}

test.describe('control messages sent from client to relay', () => {
  test('relay accepts every client-initiated control message without dropping the session', async ({ browser }) => {
    const { context, client } = await arrangeMessageE2ESession(browser)

    try {
      // Assert: ローカル検証用の relay URL が初期表示されていることを確認する。
      await expect(client.urlInput).toHaveValue(messageE2EConfig.moqtUrl)

      // Act: relay へ接続する。
      await client.connectButton.click()
      // Assert: WebTransport の接続完了を待ってから制御メッセージを送る。
      await expect(client.logPanel).toContainText('[moqt][wt] connected')

      // CLIENT_SETUP は SERVER_SETUP の受信まで待ってから成功扱いになる。
      await send(client, client.setupButton, 'CLIENT_SETUP')
      await arrangeNamespaceFields(client)

      // 既存の client -> relay メッセージ。PUBLISH_NAMESPACE / SUBSCRIBE_NAMESPACE /
      // SUBSCRIBE は relay の応答を待ってから成功扱いになるため、ステータスは
      // relay が受理したことまで示す。
      await send(client, client.publishNamespaceButton, 'PUBLISH_NAMESPACE')
      await send(client, client.subscribeNamespaceButton, 'SUBSCRIBE_NAMESPACE')
      await send(client, client.subscribeButton, 'SUBSCRIBE')
      await send(client, client.unsubscribeButton, 'UNSUBSCRIBE')
      await send(client, client.publishButton, 'PUBLISH')
      await send(client, client.fetchButton, 'FETCH')
      await send(client, client.publishNamespaceDoneButton, 'PUBLISH_NAMESPACE_DONE')
      await send(client, client.unsubscribeNamespaceButton, 'UNSUBSCRIBE_NAMESPACE')

      // draft-14 で新規に実装したメッセージ。
      await send(client, client.goAwayButton, 'GOAWAY')
      await send(client, client.maxRequestIdButton, 'MAX_REQUEST_ID')
      await send(client, client.requestsBlockedButton, 'REQUESTS_BLOCKED')
      await send(client, client.fetchCancelButton, 'FETCH_CANCEL')
      await send(client, client.subscribeUpdateButton, 'SUBSCRIBE_UPDATE')
      await send(client, client.publishDoneButton, 'PUBLISH_DONE')
      await send(client, client.publishNamespaceCancelButton, 'PUBLISH_NAMESPACE_CANCEL')
      // TRACK_STATUS は最後に送る。relay からの応答が届くことで、直前の
      // メッセージ群を受信したあともセッションが生きていることが分かる。
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
