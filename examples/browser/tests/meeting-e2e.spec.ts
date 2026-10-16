import { expect, test, type Page } from '@playwright/test'
import {
  MEETING_JWT,
  arrangeMeetingClient,
  enableMedia,
  expectCatalogStatus,
  expectTrackStatus,
  getChatMessageByText,
  getMemberCard,
  getMemberStats,
  getMemberVideo,
  getTrackSelect,
  joinRoom,
  openChat,
  openMemberDetails,
  sendChatMessage
} from './meeting-e2e-arrange'

// Assert that a video element is actively playing (decoded frames, time advancing, not paused).
async function expectVideoPlaying(video: import('@playwright/test').Locator): Promise<void> {
  // Assert: video 要素がデコード済みデータを持つ readyState に達していることを確認する。
  await expect
    .poll(async () => video.evaluate((el) => (el as HTMLVideoElement).readyState), { timeout: 30_000 })
    .toBeGreaterThanOrEqual(2)
  // Assert: 再生時刻が進み、映像が実際に流れていることを確認する。
  await expect
    .poll(async () => video.evaluate((el) => (el as HTMLVideoElement).currentTime), { timeout: 30_000 })
    .toBeGreaterThan(0)
  // Assert: pause 状態でないことを確認する。
  await expect.poll(async () => video.evaluate((el) => (el as HTMLVideoElement).paused)).toBe(false)
}

// Assert that inter-frames decode too: the publisher sends one keyframe per second, so frames
// must keep arriving well above that rate.
async function expectInterFramesDecoded(video: import('@playwright/test').Locator): Promise<void> {
  const initialFrames = await video.evaluate(
    (el) => (el as HTMLVideoElement).getVideoPlaybackQuality().totalVideoFrames
  )
  await expect
    .poll(async () => video.evaluate((el) => (el as HTMLVideoElement).getVideoPlaybackQuality().totalVideoFrames), {
      timeout: 30_000
    })
    .toBeGreaterThanOrEqual(initialFrames + 10)
}

// Assert that the member's audio plays: the A/V offset is only reported while audio chunks are
// being scheduled on a running AudioContext alongside shown frames.
async function expectAudioPlaying(stats: import('@playwright/test').Locator): Promise<void> {
  await expect(stats).toHaveText(/A\/V [+-]\d+ ms/, { timeout: 30_000 })
}

// Wait until a remote member's catalog is fully subscribed on the viewer's screen.
async function expectCatalogSubscribed(viewer: import('@playwright/test').Page, memberName: string): Promise<void> {
  await expectCatalogStatus(viewer, memberName, 'Subscribed', 60_000)
}

// Wait until the viewer has subscribed a remote member's video+audio, which it does
// on its own while the member's tile is on the stage.
async function subscribeMedia(viewer: import('@playwright/test').Page, memberName: string): Promise<void> {
  await expectTrackStatus(viewer, 'video', memberName, 'Subscribed', 60_000)
  await expectTrackStatus(viewer, 'audio', memberName, 'Subscribed', 60_000)
}

// Assert a remote member's video is playing and audio is being heard.
async function expectMediaFlowing(viewer: import('@playwright/test').Page, memberName: string): Promise<void> {
  await expectVideoPlaying(getMemberVideo(viewer, memberName))
  await expectAudioPlaying(getMemberStats(viewer, memberName))
}

// Subscribe to a remote member's video+audio and assert both are actually flowing.
async function subscribeAndExpectMedia(viewer: import('@playwright/test').Page, memberName: string): Promise<void> {
  await subscribeMedia(viewer, memberName)
  await expectMediaFlowing(viewer, memberName)
}

test('cross-relay: two clients on different relays see each other and receive video+audio', async ({ browser }) => {
  // Arrange: relay-a に接続する Client1 と relay-b に接続する Client2 を用意する。
  const client1 = await arrangeMeetingClient(browser)
  const client2 = await arrangeMeetingClient(browser)

  try {
    const roomName = `e2e-cross-relay-${Date.now()}`

    // Act: Client1 を relay-a 経由で同じルームに参加させカメラ・マイクを有効にする。
    await joinRoom(client1.page, roomName, 'alice', 'a')
    await enableMedia(client1.page)
    // Assert: Client1 のルームヘッダーにルーム名が表示されることを確認する。
    await expect(client1.page.roomName).toHaveText(roomName)

    // Act: Client2 を relay-b 経由で同じルームに参加させカメラ・マイクを有効にする。
    await joinRoom(client2.page, roomName, 'bob', 'b')
    await enableMedia(client2.page)
    // Assert: Client2 のルームヘッダーにルーム名が表示されることを確認する。
    await expect(client2.page.roomName).toHaveText(roomName)

    // Assert: Client1 側に bob のメンバーカードが現れ、カタログが自動購読されることを確認する。
    await expectCatalogStatus(client1.page.page, 'bob', 'Subscribed', 60_000)
    // Assert: Client2 側に alice のメンバーカードが現れ、カタログが自動購読されることを確認する。
    await expectCatalogStatus(client2.page.page, 'alice', 'Subscribed', 60_000)

    // Act: Client1/Client2 が相手の映像・音声を明示的に購読する。
    await subscribeMedia(client1.page.page, 'bob')
    await subscribeMedia(client2.page.page, 'alice')

    // Assert: Client1 が bob の映像を再生していることを確認する（クロスリレー経路）。
    await expectVideoPlaying(getMemberVideo(client1.page.page, 'bob'))
    // Assert: Client1 が bob の音声を再生していることを確認する。
    await expectAudioPlaying(getMemberStats(client1.page.page, 'bob'))

    // Assert: Client2 が alice の映像を再生していることを確認する（クロスリレー経路）。
    await expectVideoPlaying(getMemberVideo(client2.page.page, 'alice'))
    // Assert: Client2 が alice の音声を再生していることを確認する。
    await expectAudioPlaying(getMemberStats(client2.page.page, 'alice'))

    // Assert: both directions decode inter-frames, not only keyframes.
    await expectInterFramesDecoded(getMemberVideo(client1.page.page, 'bob'))
    await expectInterFramesDecoded(getMemberVideo(client2.page.page, 'alice'))
  } finally {
    await client1.context.close()
    await client2.context.close()
  }
})

test('leave and rejoin with same memberName succeeds without stale-member failure', async ({ browser }) => {
  // Arrange: 同一ルームに relay-a 経由で 2 クライアントを参加させる。
  const client1 = await arrangeMeetingClient(browser)
  const client2 = await arrangeMeetingClient(browser)

  try {
    const roomName = `e2e-rejoin-${Date.now()}`

    // Act: Client1 を relay-a 経由でルームに参加させカメラ・マイクを有効にする。
    await joinRoom(client1.page, roomName, 'alice', 'a')
    await enableMedia(client1.page)

    // Act: Client2 を relay-a 経由で同じルームに参加させカメラ・マイクを有効にする。
    await joinRoom(client2.page, roomName, 'bob', 'a')
    await enableMedia(client2.page)

    // Assert: 双方に相手が現れカタログ購読が完了することを確認する。
    await expectCatalogSubscribed(client1.page.page, 'bob')
    await expectCatalogSubscribed(client2.page.page, 'alice')

    // Act: Client2 が映像・音声を購読し再生が始まることを確認する。
    await subscribeMedia(client2.page.page, 'alice')
    await expectVideoPlaying(getMemberVideo(client2.page.page, 'alice'))

    // Act: Client2 がルームを退室する（Leave Room ボタン）。
    await client2.page.leaveButton.click()
    // Assert: Join フォームに戻り、Client1 側から古い bob のメンバー表示が消える。
    await expect(client2.page.roomName).not.toBeVisible({ timeout: 15_000 })
    await expect(getMemberCard(client1.page.page, 'bob')).not.toBeVisible({ timeout: 15_000 })

    // Act: Client2 が同じ名前・同じルームで再参加する。
    await joinRoom(client2.page, roomName, 'bob', 'a')
    await enableMedia(client2.page)
    // Assert: 再参加後もルームヘッダーにルーム名が表示されることを確認する。
    await expect(client2.page.roomName).toHaveText(roomName)

    // Assert: 再参加後も双方に相手が現れカタログ購読されることを確認する。
    await expectCatalogSubscribed(client1.page.page, 'bob')
    await expectCatalogSubscribed(client2.page.page, 'alice')

    // Assert: 再参加後も双方向で映像・音声が流れることを確認する。
    await subscribeAndExpectMedia(client2.page.page, 'alice')
    await subscribeAndExpectMedia(client1.page.page, 'bob')
  } finally {
    await client1.context.close()
    await client2.context.close()
  }
})

test('three clients in the same room all see each other and receive video+audio', async ({ browser }) => {
  // Arrange: relay-a に alice/carol、relay-b に bob を参加させる 3 クライアントを用意する。
  const alice = await arrangeMeetingClient(browser)
  const bob = await arrangeMeetingClient(browser)
  const carol = await arrangeMeetingClient(browser)

  try {
    const roomName = `e2e-trio-${Date.now()}`

    // Act: 3 名を同じルームに参加させ、それぞれカメラ・マイクを有効にする。
    await joinRoom(alice.page, roomName, 'alice', 'a')
    await enableMedia(alice.page)
    await joinRoom(bob.page, roomName, 'bob', 'b')
    await enableMedia(bob.page)
    await joinRoom(carol.page, roomName, 'carol', 'a')
    await enableMedia(carol.page)

    // Assert: 各クライアントが他の 2 名をリモートメンバーとして認識しカタログ購読が完了する。
    await Promise.all([
      expectCatalogSubscribed(alice.page.page, 'bob'),
      expectCatalogSubscribed(alice.page.page, 'carol'),
      expectCatalogSubscribed(bob.page.page, 'alice'),
      expectCatalogSubscribed(bob.page.page, 'carol'),
      expectCatalogSubscribed(carol.page.page, 'alice'),
      expectCatalogSubscribed(carol.page.page, 'bob')
    ])

    // Act: 全 6 経路を先に一括購読する（配信開始直後に購読し直近キーフレームを拾うため）。
    await subscribeMedia(alice.page.page, 'bob')
    await subscribeMedia(alice.page.page, 'carol')
    await subscribeMedia(bob.page.page, 'alice')
    await subscribeMedia(bob.page.page, 'carol')
    await subscribeMedia(carol.page.page, 'alice')
    await subscribeMedia(carol.page.page, 'bob')

    // Assert: 各クライアントが両相手から映像・音声を受信できることをまとめて確認する。
    await expectMediaFlowing(alice.page.page, 'bob')
    await expectMediaFlowing(alice.page.page, 'carol')
    await expectMediaFlowing(bob.page.page, 'alice')
    await expectMediaFlowing(bob.page.page, 'carol')
    await expectMediaFlowing(carol.page.page, 'alice')
    await expectMediaFlowing(carol.page.page, 'bob')
  } finally {
    await alice.context.close()
    await bob.context.close()
    await carol.context.close()
  }
})

test('repeated leave and rejoin keeps reconnecting with the same memberName', async ({ browser }) => {
  // Arrange: relay-a に alice と bob を参加させる。
  const alice = await arrangeMeetingClient(browser)
  const bob = await arrangeMeetingClient(browser)

  try {
    const roomName = `e2e-rejoin-loop-${Date.now()}`

    // Act: alice と bob を同じルームに参加させカメラ・マイクを有効にする。
    await joinRoom(alice.page, roomName, 'alice', 'a')
    await enableMedia(alice.page)
    await joinRoom(bob.page, roomName, 'bob', 'a')
    await enableMedia(bob.page)
    // Assert: 双方に相手が現れカタログ購読が完了する。
    await expectCatalogSubscribed(alice.page.page, 'bob')
    await expectCatalogSubscribed(bob.page.page, 'alice')

    // Act / Assert: bob が退室→同名再参加を 3 回繰り返し、毎回問題なく再接続できることを確認する。
    for (let attempt = 1; attempt <= 3; attempt++) {
      // Act: bob がルームを退室する。
      await bob.page.leaveButton.click()
      // Assert: Join フォームに戻り、alice 側から古い bob のメンバー表示が消える。
      await expect(bob.page.roomName).not.toBeVisible({ timeout: 15_000 })
      await expect(getMemberCard(alice.page.page, 'bob')).not.toBeVisible({ timeout: 15_000 })

      // Act: bob が同じ名前・同じルームに再参加する。
      await joinRoom(bob.page, roomName, 'bob', 'a')
      await enableMedia(bob.page)
      // Assert: 再参加のたびにルームヘッダーが表示される。
      await expect(bob.page.roomName).toHaveText(roomName)
      // Assert: 双方で相手が毎回リモートメンバーとして再認識される。
      await expectCatalogSubscribed(alice.page.page, 'bob')
      await expectCatalogSubscribed(bob.page.page, 'alice')

      // Assert: 各再参加直後に双方向で映像・音声が流れることを確認する。
      await subscribeAndExpectMedia(bob.page.page, 'alice')
      await subscribeAndExpectMedia(alice.page.page, 'bob')
    }
  } finally {
    await alice.context.close()
    await bob.context.close()
  }
})

test('a viewer switches a member camera between renditions on the same picture', async ({ browser }) => {
  // Arrange: alice publishes a 160x90 and a 106x60 rendition of one codec; bob plays the first
  const params = { e2eVideoCodec: 'avc1.42001F', e2eVideoLowHeight: '60' }
  const alice = await arrangeMeetingClient(browser, { params })
  const bob = await arrangeMeetingClient(browser, { params })
  const videoWidth = (video: import('@playwright/test').Locator) =>
    video.evaluate((element) => (element as HTMLVideoElement).videoWidth)

  try {
    const roomName = `e2e-switch-${Date.now()}`
    await joinRoom(alice.page, roomName, 'alice', 'a')
    await enableMedia(alice.page)
    await joinRoom(bob.page, roomName, 'bob', 'a')
    await expectCatalogSubscribed(bob.page.page, 'alice')
    await subscribeMedia(bob.page.page, 'alice')
    const video = getMemberVideo(bob.page.page, 'alice')
    await expectVideoPlaying(video)
    await expect.poll(async () => videoWidth(video), { timeout: 30_000 }).toBe(160)
    await video.evaluate((element) => {
      element.dataset.beforeSwitch = 'true'
    })

    // Act
    await openMemberDetails(bob.page.page, 'alice')
    await getTrackSelect(bob.page.page, 'video', 'alice').selectOption('camera_e2e_e2e_low')

    // Assert: the same picture element plays the lower rendition, so the pipeline was kept
    await expect.poll(async () => videoWidth(video), { timeout: 30_000 }).toBe(106)
    await expect(video).toHaveAttribute('data-before-switch', 'true')
    await expectTrackStatus(bob.page.page, 'video', 'alice', 'Subscribed')
    await expectInterFramesDecoded(video)

    // Act
    await getTrackSelect(bob.page.page, 'video', 'alice').selectOption('@auto')

    // Assert: the tile is taller than 90 px and delivery keeps up, so Auto steps back up after its calm spell
    await expect.poll(async () => videoWidth(video), { timeout: 45_000 }).toBe(160)
    await expect(video).toHaveAttribute('data-before-switch', 'true')
  } finally {
    await alice.context.close()
    await bob.context.close()
  }
})

test('Auto follows the size of a member tile when the view changes', async ({ browser }) => {
  // Arrange: bob publishes 640x360 and 320x180, a filmstrip tile is 126 px tall, and carol speaks so she takes the focus
  const params = {
    e2eVideoCodec: 'avc1.42001F',
    e2eVideoWidth: '640',
    e2eVideoHeight: '360',
    e2eVideoLowHeight: '180',
    e2eVideoBitrate: '300000'
  }
  const alice = await arrangeMeetingClient(browser, { params })
  const bob = await arrangeMeetingClient(browser, { params })
  const carol = await arrangeMeetingClient(browser, { params })
  const videoWidth = (video: import('@playwright/test').Locator) =>
    video.evaluate((element) => (element as HTMLVideoElement).videoWidth)

  try {
    const roomName = `e2e-tile-size-${Date.now()}`
    await joinRoom(alice.page, roomName, 'alice', 'a')
    await joinRoom(bob.page, roomName, 'bob', 'a')
    await enableMedia(bob.page)
    await bob.page.toggleMicrophoneButton.click()
    await joinRoom(carol.page, roomName, 'carol', 'a')
    await enableMedia(carol.page)
    const bobVideo = getMemberVideo(alice.page.page, 'bob')
    await expect.poll(async () => videoWidth(bobVideo), { timeout: 30_000 }).toBe(640)
    await expect(alice.page.page.getByTestId('member-chip-carol')).toHaveClass(/ring-emerald-400/, { timeout: 20_000 })

    // Act
    await alice.page.page.getByTestId('stage-view-button').click()

    // Assert: bob's filmstrip tile is shorter than 180 px, so Auto drops him to 320x180
    await expect.poll(async () => videoWidth(bobVideo), { timeout: 20_000 }).toBe(320)

    // Act
    await alice.page.page.getByTestId('stage-view-button').click()

    // Assert: the gallery tile raises the ceiling and Auto takes it without the calm spell of a network step up
    await expect.poll(async () => videoWidth(bobVideo), { timeout: 8_000 }).toBe(640)
  } finally {
    await alice.context.close()
    await bob.context.close()
    await carol.context.close()
  }
})

test('closing the filmstrip unsubscribes the video of the members it hid', async ({ browser }) => {
  // Arrange: carol turns her microphone off, so alice's speaker view focuses bob
  const alice = await arrangeMeetingClient(browser)
  const bob = await arrangeMeetingClient(browser)
  const carol = await arrangeMeetingClient(browser)

  try {
    const roomName = `e2e-filmstrip-${Date.now()}`
    await joinRoom(alice.page, roomName, 'alice', 'a')
    await joinRoom(bob.page, roomName, 'bob', 'a')
    await enableMedia(bob.page)
    await joinRoom(carol.page, roomName, 'carol', 'a')
    await enableMedia(carol.page)
    await carol.page.toggleMicrophoneButton.click()
    await subscribeMedia(alice.page.page, 'bob')
    await expectTrackStatus(alice.page.page, 'video', 'carol', 'Subscribed', 60_000)
    // Assert: the beeps of bob's fake microphone carry voice activity in their LOC Audio Level
    await expect(alice.page.page.getByTestId('member-chip-bob')).toHaveClass(/ring-emerald-400/, { timeout: 20_000 })
    await alice.page.page.getByTestId('stage-view-button').click()

    // Act
    await alice.page.page.getByTestId('filmstrip-button').click()

    // Assert: only the focused speaker's video is still received, and his audio with it
    await expectTrackStatus(alice.page.page, 'video', 'carol', 'Not subscribed')
    await expectTrackStatus(alice.page.page, 'video', 'bob', 'Subscribed')
    await expectTrackStatus(alice.page.page, 'audio', 'bob', 'Subscribed')

    // Act
    await alice.page.page.getByTestId('filmstrip-button').click()

    // Assert
    await expectTrackStatus(alice.page.page, 'video', 'carol', 'Subscribed', 60_000)
  } finally {
    await alice.context.close()
    await bob.context.close()
    await carol.context.close()
  }
})

test('multiple clients on a single relay all connect and receive video+audio', async ({ browser }) => {
  // Arrange: 全員 relay-a に接続する 3 クライアントを用意する（単一リレーで複数接続を検証）。
  const alice = await arrangeMeetingClient(browser)
  const bob = await arrangeMeetingClient(browser)
  const carol = await arrangeMeetingClient(browser)

  try {
    const roomName = `e2e-single-relay-${Date.now()}`

    // Act: 3 名すべてを relay-a 経由で同じルームに参加させ、カメラ・マイクを有効にする。
    await joinRoom(alice.page, roomName, 'alice', 'a')
    await enableMedia(alice.page)
    await joinRoom(bob.page, roomName, 'bob', 'a')
    await enableMedia(bob.page)
    await joinRoom(carol.page, roomName, 'carol', 'a')
    await enableMedia(carol.page)

    // Assert: 同一リレー上で各クライアントが他の 2 名を認識しカタログ購読が完了する。
    await Promise.all([
      expectCatalogSubscribed(alice.page.page, 'bob'),
      expectCatalogSubscribed(alice.page.page, 'carol'),
      expectCatalogSubscribed(bob.page.page, 'alice'),
      expectCatalogSubscribed(bob.page.page, 'carol'),
      expectCatalogSubscribed(carol.page.page, 'alice'),
      expectCatalogSubscribed(carol.page.page, 'bob')
    ])

    // Act: 単一リレー経由で全 6 経路を先に一括購読する。
    await subscribeMedia(alice.page.page, 'bob')
    await subscribeMedia(alice.page.page, 'carol')
    await subscribeMedia(bob.page.page, 'alice')
    await subscribeMedia(bob.page.page, 'carol')
    await subscribeMedia(carol.page.page, 'alice')
    await subscribeMedia(carol.page.page, 'bob')

    // Assert: 全員が相互に映像・音声を受信できることをまとめて確認する。
    await expectMediaFlowing(alice.page.page, 'bob')
    await expectMediaFlowing(alice.page.page, 'carol')
    await expectMediaFlowing(bob.page.page, 'alice')
    await expectMediaFlowing(bob.page.page, 'carol')
    await expectMediaFlowing(carol.page.page, 'alice')
    await expectMediaFlowing(carol.page.page, 'bob')
  } finally {
    await alice.context.close()
    await bob.context.close()
    await carol.context.close()
  }
})

test('chat message sent by one client is displayed on the receiving client', async ({ browser }) => {
  // Arrange: relay-a に alice と bob を参加させる。
  const alice = await arrangeMeetingClient(browser)
  const bob = await arrangeMeetingClient(browser)

  try {
    const roomName = `e2e-chat-${Date.now()}`

    // Act: 2 名を同じルームに参加させカメラ・マイクを有効にする。
    await joinRoom(alice.page, roomName, 'alice', 'a')
    await enableMedia(alice.page)
    await joinRoom(bob.page, roomName, 'bob', 'a')
    await enableMedia(bob.page)

    // Assert: 互いを認識しカタログ購読が完了する。
    await expectCatalogSubscribed(alice.page.page, 'bob')
    await expectCatalogSubscribed(bob.page.page, 'alice')
    // Assert: 送信前に bob が alice の chat トラックを自動購読し終えるまで待つ（メッセージ取りこぼし防止）。
    await expectTrackStatus(bob.page.page, 'chat', 'alice', 'Subscribed', 60_000)

    // Act: alice が一意なチャットメッセージを送信する。
    const message = `hello-from-alice-${Date.now()}`
    await sendChatMessage(alice.page, message)

    // Assert: 受信側 bob のチャットに alice からのメッセージが表示される。
    await openChat(bob.page)
    await expect(getChatMessageByText(bob.page.page, message)).toBeVisible({ timeout: 30_000 })
    // Assert: 送信側 alice のチャットにも自分のメッセージが表示される。
    await expect(getChatMessageByText(alice.page.page, message)).toBeVisible()
  } finally {
    await alice.context.close()
    await bob.context.close()
  }
})

test('an invalid token is rejected by the relay with a surfaced reason', async ({ browser }) => {
  // Arrange: 不正な JWT を提示するクライアントを用意する。
  const client = await arrangeMeetingClient(browser, { jwt: 'not-a-jwt' })

  try {
    // Act: 不正トークンのまま relay-a に入室を試みる。
    await client.page.roomNameInput.fill(`e2e-auth-${Date.now()}`)
    await client.page.userNameInput.fill('mallory')
    await client.page.localRelayTab.click()
    await client.page.relayARadio.click()
    await client.page.submitButton.click()

    // Assert: relay が CLIENT_SETUP を拒否し、理由付きの入室エラーが表示されてルーム画面に進まないことを確認する。
    await expect(client.page.joinError).toBeVisible({ timeout: 30_000 })
    await expect(client.page.joinError).toContainText('code 2')
    await expect(client.page.roomName).toHaveCount(0)
  } finally {
    await client.context.close()
  }
})
