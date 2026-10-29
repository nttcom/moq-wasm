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
  // Assert: Publisher がローカル検証用の接続先 URL で初期化されていることを確認する。
  await expect(publisher.urlInput).toHaveValue(mediaE2EConfig.moqtUrl)
  // Assert: Subscriber も同じ接続先 URL を参照していることを確認する。
  await expect(subscriber.urlInput).toHaveValue(mediaE2EConfig.moqtUrl)
  // Assert: Publisher が送信先と同じ namespace を初期表示していることを確認する。
  await expect(publisher.namespaceInput).toHaveValue(mediaE2EConfig.namespace)
  // Assert: Subscriber も同じ namespace を購読対象として初期表示していることを確認する。
  await expect(subscriber.namespaceInput).toHaveValue(mediaE2EConfig.namespace)
}

async function startPublisherBroadcast(publisher: PublisherPageModel): Promise<void> {
  // Act: Publisher を MoQT サーバーへ接続する。
  await publisher.connectButton.click()
  // Assert: Publisher 側で接続完了ステータスが表示されることを確認する。
  await expect(publisher.connectionStatus).toContainText('Connected:')
  // Assert: 期待した transport（既定は WebTransport、TCP proxy 経由なら WebSocket）で繋がったことを確認する。
  await expect.poll(() => publisher.sessionTransports).toEqual([mediaE2EConfig.expectedTransport])

  // Act: fake media を使って Publisher の getUserMedia を開始する。
  await publisher.getUserMediaButton.click()
  // Assert: キャプチャ準備が整い、送信に使えるメディアが取得できたことを確認する。
  await expect(publisher.captureStatus).toContainText('Media ready')

  // Act: 取得したメディアを broadcast として publish する。
  await publisher.sendButton.click()
  // Assert: 対象 namespace の publish が始まったことを確認する。
  await expect(publisher.sendStatus).toContainText(`Publishing started: ${mediaE2EConfig.namespace}`)
}

async function subscribeForPlayback(subscriber: SubscriberPageModel): Promise<void> {
  // Act: Subscriber を同じ MoQT サーバーへ接続する。
  await subscriber.connectButton.click()
  // Assert: Subscriber 側でも接続完了ステータスが表示されることを確認する。
  await expect(subscriber.connectionStatus).toContainText('Connected:')

  // Act: Subscriber の setup を送信して購読準備を整える。
  await subscriber.setupButton.click()
  // Assert: setup が受理され、catalog 購読に進める状態になったことを確認する。
  await expect(subscriber.setupStatus).toContainText('Setup acknowledged')
  // Assert: Subscriber も期待した transport で繋がったことを確認する。
  await expect.poll(() => subscriber.sessionTransports).toEqual([mediaE2EConfig.expectedTransport])

  // Act: catalog を購読して配信中トラックの一覧を取得する。
  await subscriber.catalogSubscribeButton.click()
  // Assert: catalog が読み込まれ、購読候補のトラック情報が表示されることを確認する。
  await expect(subscriber.catalogStatus).toContainText('Catalog loaded:')

  // Act: 映像トラックの subscribe を開始する。
  await subscriber.trackSubscribeButton.click()
}

async function expectSubscriberPlayback(subscriber: SubscriberPageModel): Promise<void> {
  // Assert: Subscriber が少なくとも 1 つ以上の video object を受信したことを確認する。
  await expect(subscriber.receiveStatus).toHaveText(/Received video objects: [1-9]/)
  // Assert: UI 上で再生中ステータスに遷移していることを確認する。
  await expect(subscriber.playbackStatus).toContainText('Playing')

  // Assert: video 要素がデコード済みデータを持つ readyState まで進んだことを確認する。
  await expect.poll(async () => subscriber.video.evaluate((video) => video.readyState)).toBeGreaterThanOrEqual(2)
  // Assert: 再生時刻が進み、実際に映像再生が進行していることを確認する。
  await expect.poll(async () => subscriber.video.evaluate((video) => video.currentTime)).toBeGreaterThan(0)
  // Assert: video 要素が pause 状態ではなく再生中であることを確認する。
  await expect.poll(async () => subscriber.video.evaluate((video) => video.paused)).toBe(false)

  // Assert: 最終的な video 要素の状態でも再生進行が確認できることを裏取りする。
  const playback = await subscriber.video.evaluate((video) => ({
    currentTime: video.currentTime,
    readyState: video.readyState
  }))
  expect(playback.readyState).toBeGreaterThanOrEqual(2)
  expect(playback.currentTime).toBeGreaterThan(0)
}

test('publisher streams video to subscriber over local MoQT server', async ({ browser }) => {
  // Arrange: Publisher と Subscriber を同じ URL / namespace で開いたテストセッションを用意する。
  const session = await arrangeMediaE2ESession(browser)

  try {
    // Assert: 2 画面ともローカル E2E 用の初期値で立ち上がっていることを確認する。
    await expectInitialConfiguration(session.publisher, session.subscriber)

    // Act / Assert: Publisher 側で接続からメディアの publish 開始までを完了する。
    await startPublisherBroadcast(session.publisher)

    // Act / Assert: Subscriber 側で接続から track subscribe までを完了する。
    await subscribeForPlayback(session.subscriber)

    // Assert: Subscriber が映像を受信し、video 要素の再生が進行していることを確認する。
    await expectSubscriberPlayback(session.subscriber)
  } finally {
    await session.context.close()
  }
})
