import { expect, test, type Locator } from '@playwright/test'
import { arrangeLiveViewerE2ESession, type LiveViewerPageModel } from './live-viewer-e2e-arrange'

test('live viewer plays the ingested stream, switches renditions and rewinds', async ({ browser }) => {
  // Arrange
  const { context, viewer } = await arrangeLiveViewerE2ESession(browser)

  try {
    // Assert
    await expect(viewer.seekbar).toBeDisabled()
    await expect(viewer.seekElapsed).toHaveText('--:-- / --:--')

    // Act
    await viewer.watchButton.click()

    // Assert
    await expect(viewer.connectionStatus).toContainText('Connected:')
    await expect(viewer.catalogStatus).toContainText(/Catalog loaded: [1-9]/)
    await expect(viewer.playbackStatus).toContainText('Playing')
    await expect(viewer.liveButton).not.toHaveClass(/reviewing/)
    await expectVideoDecoded(viewer)

    // Act: relay のキャッシュが 10 秒分たまるのを待って巻き戻す
    await expect.poll(async () => parseSeconds(viewer.rewindBuffer), { timeout: 60_000 }).toBeGreaterThan(10)
    await expect(viewer.seekbar).toBeEnabled()
    await expect(viewer.seekPosition).toHaveText('LIVE')
    await expect(viewer.seekElapsed).toHaveText(/^\d+:\d{2} \/ \d+:\d{2}$/)

    // Assert: 軸は配信開始から始まり、FETCH できる区間はその一部として示される
    await expect(viewer.seekStart).toHaveText(/^0:0\d$/)
    const replayable = await viewer.seekAvailableWindow.evaluate((element) => ({
      offset: Number.parseFloat((element as HTMLElement).style.left),
      width: Number.parseFloat((element as HTMLElement).style.width)
    }))
    expect(replayable.offset).toBeGreaterThan(0)
    expect(replayable.width).toBeLessThan(100)

    await viewer.seekbar.focus()
    await viewer.seekbar.press('Home')

    // Assert
    await expect(viewer.rewindStatus).toContainText(/Rewound \d/)
    await expect(viewer.playbackStatus).toContainText('Reviewing')
    await expect(viewer.liveButton).toHaveClass(/reviewing/)
    await expect(viewer.reviewCanvas).toBeVisible()
    await expect
      .poll(async () => viewer.reviewCanvas.evaluate((element) => (element as HTMLCanvasElement).width), {
        timeout: 30_000
      })
      .toBe(await viewer.video.evaluate((element) => (element as HTMLVideoElement).videoWidth))

    await expect(viewer.seekPosition).toHaveText(/-\d+\.\ds/)

    // Assert: MSF media timeline が配信開始からの経過を返し、シーク位置がライブより手前になる
    const [atPosition, broadcast] = (await viewer.seekElapsed.innerText()).split(' / ').map(parseClock)
    expect(atPosition).toBeLessThan(broadcast)

    const thumbBefore = await thumbValue(viewer)
    const playedBefore = await playedWidth(viewer)

    // Assert: 1 ウィンドウ分 (4 group = 8 秒) を超えて再生が続く
    await expect
      .poll(async () => elapsedAtPosition(await viewer.seekElapsed.innerText()), { timeout: 30_000 })
      .toBeGreaterThan(atPosition + 9)
    await expect(viewer.logPanel).toContainText(/fetched \d+ objects/)

    // Assert: つまみはシークした位置に留まり、再生の進みは塗りが表す
    expect(await thumbValue(viewer)).toBeCloseTo(thumbBefore, 1)
    expect(await playedWidth(viewer)).toBeGreaterThan(playedBefore)

    // Act
    await viewer.seekbar.press('End')

    // Assert
    await expect(viewer.rewindStatus).toContainText('Live')
    await expect(viewer.seekPosition).toHaveText('LIVE')
    await expect(viewer.liveButton).not.toHaveClass(/reviewing/)
    await expect(viewer.video).toBeVisible()
    await expect(viewer.playbackStatus).toContainText('Playing')

    // Act
    const slider = await viewer.seekbar.boundingBox()
    expect(slider).not.toBeNull()
    await viewer.page.mouse.move(slider!.x + slider!.width - 8, slider!.y + slider!.height / 2)
    await viewer.page.mouse.down()
    await viewer.page.mouse.move(slider!.x + slider!.width / 2, slider!.y + slider!.height / 2, { steps: 5 })

    // Assert
    await expect(viewer.seekPosition).toHaveText(/-\d+\.\ds/)
    await expect(viewer.rewindStatus).toHaveText('Live')

    // Act
    await viewer.page.mouse.up()

    // Assert
    await expect(viewer.rewindStatus).toContainText(/Rewound \d/)

    // Act: ↓ でライブから 5 秒戻り、→ で 1 秒進む
    await viewer.liveButton.click()
    await viewer.page.keyboard.press('ArrowDown')

    // Assert: つまみは目標位置そのものに置かれ、描画は目標以降のフレームから始まる
    await expect(viewer.playbackStatus).toContainText('Reviewing')
    await expect(viewer.seekPosition).toHaveText(/-[5-7]\.\ds/)
    const skippedTo = await thumbValue(viewer)
    expect(skippedTo).toBeLessThan(Number(await viewer.seekbar.getAttribute('max')))
    await expect
      .poll(async () => elapsedAtPosition(await viewer.seekElapsed.innerText()), { timeout: 15_000 })
      .toBeGreaterThan(0)

    // Act
    await viewer.page.keyboard.press('ArrowRight')

    // Assert
    await expect.poll(async () => thumbValue(viewer)).toBeGreaterThan(skippedTo)

    // Act: 映像に重なるボタンはカーソルを合わせると押せる
    const beforeBack = await thumbValue(viewer)
    await viewer.seekbar.hover()
    await viewer.skipBack5Button.click()

    // Assert
    await expect.poll(async () => thumbValue(viewer)).toBeLessThan(beforeBack)

    // Act: ↑ を 2 回でライブ端を越える
    await viewer.page.keyboard.press('ArrowUp')
    await viewer.page.keyboard.press('ArrowUp')

    // Assert
    await expect(viewer.seekPosition).toHaveText('LIVE')
    await expect(viewer.reviewCanvas).toBeHidden()

    // Act: 歯車から下位画質へ切り替える
    await viewer.qualityButton.click()
    await expect(viewer.qualityMenu).toBeVisible()
    const renditions = await viewer.videoTrackSelect.locator('option').allInnerTexts()
    expect(renditions.length).toBeGreaterThan(1)
    await viewer.videoTrackSelect.selectOption({ index: 1 })

    // Assert: 元の track を解除して rendition を購読し直す
    await expect(viewer.logPanel).toContainText('unsubscribed video')
    await expect(viewer.logPanel).toContainText(/subscribed \S*\/video_\d+p/)
    await expect(viewer.seekPosition).toHaveText('LIVE')

    // Act
    await viewer.stopButton.click()

    // Assert
    await expect(viewer.rewindBuffer).toHaveText('0.0s')
    await expect(viewer.seekbar).toBeDisabled()
    await expect(viewer.seekPosition).toHaveText('LIVE')
    await expect(viewer.seekElapsed).toHaveText('--:-- / --:--')
  } finally {
    await context.close()
  }
})

test('live viewer plays and reviews CMAF tracks through MSE', async ({ browser }) => {
  // Arrange
  const { context, viewer } = await arrangeLiveViewerE2ESession(browser)

  try {
    // Act
    await viewer.watchButton.click()
    await expect(viewer.playbackStatus).toContainText('Playing')
    await viewer.qualityButton.click()
    await viewer.packagingSelect.selectOption('cmaf')

    // Assert: the element now plays a MediaSource fed with CMAF fragments
    await expect(viewer.logPanel).toContainText(/subscribed \S*\/video_cmaf/)
    await expect.poll(async () => mediaProp(viewer.video, 'src')).toMatch(/^blob:/)
    await expectVideoDecoded(viewer)
    await expect.poll(async () => mediaProp(viewer.video, 'currentTime'), { timeout: 15_000 }).toBeGreaterThan(1)

    // Act: relay のキャッシュがたまるのを待って MSE 経由で巻き戻す
    await expect.poll(async () => parseSeconds(viewer.rewindBuffer), { timeout: 60_000 }).toBeGreaterThan(10)
    await viewer.page.keyboard.press('Escape')
    await viewer.seekbar.focus()
    await viewer.seekbar.press('Home')

    // Assert: review plays from its own MediaSource on the review element while the live one keeps running hidden
    await expect(viewer.rewindStatus).toContainText(/Rewound \d/)
    await expect(viewer.playbackStatus).toContainText('Reviewing')
    await expect(viewer.reviewCanvas).toBeHidden()
    await expect(viewer.reviewVideo).toBeVisible({ timeout: 20_000 })
    await expect(viewer.video).toBeHidden()
    await expect.poll(async () => mediaProp(viewer.reviewVideo, 'currentTime'), { timeout: 30_000 }).toBeGreaterThan(3)
    await expect(viewer.logPanel).toContainText(/fetched \d+ objects from group/)
    const liveTimeWhileReviewing = await mediaProp(viewer.video, 'currentTime')

    // Act: 巻き戻し中だけ倍速を選べる
    await expect(viewer.speedSelect).toBeEnabled()
    await viewer.speedSelect.selectOption('2')

    // Assert
    await expect.poll(async () => mediaProp(viewer.reviewVideo, 'playbackRate')).toBe(2)

    // Act
    await viewer.liveButton.click()

    // Assert: the live element was playing all along, so it is shown as is
    await expect(viewer.rewindStatus).toContainText('Live')
    await expect(viewer.liveButton).not.toHaveClass(/reviewing/)
    await expect(viewer.speedSelect).toBeDisabled()
    await expect(viewer.reviewVideo).toBeHidden()
    await expect(viewer.video).toBeVisible()
    expect(await mediaProp(viewer.video, 'src')).toMatch(/^blob:/)
    expect(await mediaProp(viewer.video, 'currentTime')).toBeGreaterThan(liveTimeWhileReviewing)
  } finally {
    await context.close()
  }
})

async function mediaProp<K extends 'src' | 'currentTime' | 'playbackRate'>(
  video: Locator,
  key: K
): Promise<HTMLVideoElement[K]> {
  return video.evaluate((element, property) => (element as HTMLVideoElement)[property], key)
}

async function expectVideoDecoded(viewer: LiveViewerPageModel): Promise<void> {
  await expect
    .poll(async () => viewer.video.evaluate((element) => (element as HTMLVideoElement).readyState), {
      timeout: 30_000
    })
    .toBeGreaterThanOrEqual(2)
  await expect
    .poll(async () => viewer.video.evaluate((element) => (element as HTMLVideoElement).videoWidth))
    .toBeGreaterThan(0)
}

async function parseSeconds(locator: Locator): Promise<number> {
  return Number.parseFloat((await locator.innerText()).replace('s', ''))
}

function parseClock(text: string): number {
  return text.split(':').reduce((total, part) => total * 60 + Number(part), 0)
}

function elapsedAtPosition(seekElapsed: string): number {
  return parseClock(seekElapsed.split(' / ')[0])
}

async function thumbValue(viewer: LiveViewerPageModel): Promise<number> {
  return viewer.seekbar.evaluate((element) => (element as HTMLInputElement).valueAsNumber)
}

async function playedWidth(viewer: LiveViewerPageModel): Promise<number> {
  return viewer.seekReviewProgress.evaluate((element) => Number.parseFloat((element as HTMLElement).style.width))
}
