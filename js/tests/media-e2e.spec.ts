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

async function connectPublisherForStreaming(publisher: PublisherPageModel): Promise<void> {
  // Act: Publisher を MoQT サーバーへ接続する。
  await publisher.connectButton.click()
  // Assert: Publisher 側で接続完了ステータスが表示されることを確認する。
  await expect(publisher.connectionStatus).toContainText('Connected:')

  // Act: Publisher の setup を送信してセッション初期化を進める。
  await publisher.setupButton.click()
  // Assert: setup が受理され、後続の announce に進める状態になったことを確認する。
  await expect(publisher.setupStatus).toContainText('Setup acknowledged')

  // Act: テスト用 namespace を announce して購読可能な状態にする。
  await publisher.announceButton.click()
  // Assert: announce 完了ステータスに対象 namespace が反映されることを確認する。
  await expect(publisher.announceStatus).toContainText(`Announced: ${mediaE2EConfig.namespace}`)
}

async function connectSubscriberForPlayback(
  publisher: PublisherPageModel,
  subscriber: SubscriberPageModel
): Promise<void> {
  // Act: Subscriber を同じ MoQT サーバーへ接続する。
  await subscriber.connectButton.click()
  // Assert: Subscriber 側でも接続完了ステータスが表示されることを確認する。
  await expect(subscriber.connectionStatus).toContainText('Connected:')

  // Act: Subscriber の setup を送信して購読準備を整える。
  await subscriber.setupButton.click()
  // Assert: setup が受理され、catalog 購読に進める状態になったことを確認する。
  await expect(subscriber.setupStatus).toContainText('Setup acknowledged')

  // Act: catalog を購読して配信中トラックの一覧を取得する。
  await subscriber.catalogSubscribeButton.click()
  // Assert: catalog が読み込まれ、購読候補のトラック情報が表示されることを確認する。
  await expect(subscriber.catalogStatus).toContainText('Catalog loaded:')

  // Act: 映像トラックの subscribe を開始する。
  await subscriber.trackSubscribeButton.click()
  // Assert: Publisher 側で購読者の準備完了が見え、送信開始可能になったことを確認する。
  await expect(publisher.sendStatus).toContainText('Subscriber ready:')
}

async function startPublisherMediaStream(publisher: PublisherPageModel): Promise<void> {
  // Act: fake media を使って Publisher の getUserMedia を開始する。
  await publisher.getUserMediaButton.click()
  // Assert: キャプチャ準備が整い、送信に使えるメディアが取得できたことを確認する。
  await expect(publisher.captureStatus).toContainText('Media ready')

  // Act: Publisher から映像送信を開始する。
  await publisher.sendButton.click()
  // Assert: 送信開始または映像ストリーミング中の状態が表示されることを確認する。
  await expect(publisher.sendStatus).toHaveText(/Publishing started|Streaming video/)
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

    // Act / Assert: Publisher 側で接続から announce までを完了し、送信準備を整える。
    await connectPublisherForStreaming(session.publisher)

    // Act / Assert: Subscriber 側で接続から track subscribe までを完了し、受信準備を整える。
    await connectSubscriberForPlayback(session.publisher, session.subscriber)

    // Act / Assert: fake media の取得と Publisher の送信開始を完了する。
    await startPublisherMediaStream(session.publisher)

    // Assert: Subscriber が映像を受信し、video 要素の再生が進行していることを確認する。
    await expectSubscriberPlayback(session.subscriber)
  } finally {
    await session.context.close()
  }
})
