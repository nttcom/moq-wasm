import { expect, test, type Page } from '@playwright/test'
import { arrangeLiveViewerE2ESession, closeLiveViewerE2ESession, expectVideoDecoded } from './live-viewer-e2e-arrange'

const relayAPid = Number(process.env.GOAWAY_E2E_RELAY_A_PID)
const STEADY_MS = 5_000
const AFTER_SWITCH_MS = 10_000
/// The runner's encoder starts a group every 2 s.
const GROUP_MS = 2_000
/// GOAWAY this long before a group starts: the publisher reaches the new relay
/// and starts the group there before the viewer's SUBSCRIBE does.
const GOAWAY_LEAD_MS = 250

type FrameWindow = { frames: number; maxGapMs: number; meanGapMs: number }

async function recordPresentedFrames(page: Page): Promise<void> {
  await page.locator('.viewer-stage video:visible').evaluate((video) => {
    const times: number[] = []
    ;(window as unknown as { presentedFrameTimes: number[] }).presentedFrameTimes = times
    const onFrame = (now: number) => {
      times.push(now)
      ;(video as HTMLVideoElement).requestVideoFrameCallback(onFrame)
    }
    ;(video as HTMLVideoElement).requestVideoFrameCallback(onFrame)
  })
}

async function presentedFrameWindow(page: Page, from: number, to: number): Promise<FrameWindow> {
  return page.evaluate(
    ([start, end]) => {
      const times = (window as unknown as { presentedFrameTimes: number[] }).presentedFrameTimes.filter(
        (time) => time >= start && time <= end
      )
      const gaps = times.slice(1).map((time, index) => time - times[index])
      return {
        frames: times.length,
        maxGapMs: Math.round(Math.max(0, ...gaps)),
        meanGapMs: Math.round(gaps.reduce((sum, gap) => sum + gap, 0) / Math.max(1, gaps.length))
      }
    },
    [from, to]
  )
}

/// Resolves when the next group of any track starts arriving.
function nextGroupStart(page: Page): Promise<void> {
  return new Promise((resolve) => {
    const onConsole = (message: { text(): string }) => {
      if (message.text().startsWith('[moqt] received subgroup header')) {
        page.off('console', onConsole)
        resolve()
      }
    }
    page.on('console', onConsole)
  })
}

function processAlive(pid: number): boolean {
  try {
    process.kill(pid, 0)
    return true
  } catch {
    return false
  }
}

test('live playback continues when the relay shuts down with GOAWAY', async ({ browser }) => {
  // Arrange: the viewer plays the stream through relay A
  expect(Number.isInteger(relayAPid)).toBe(true)
  const session = await arrangeLiveViewerE2ESession(browser)
  const { viewer } = session

  try {
    await viewer.watchButton.click()
    await expect(viewer.playbackStatus).toContainText('Playing')
    await expectVideoDecoded(viewer.video)
    await recordPresentedFrames(viewer.page)
    const steadyFrom = await viewer.page.evaluate(() => performance.now())
    await viewer.page.waitForTimeout(STEADY_MS)
    await nextGroupStart(viewer.page)
    await viewer.page.waitForTimeout(GROUP_MS - GOAWAY_LEAD_MS)

    // Act: relay A stops and sends GOAWAY naming relay B
    const stoppedAt = await viewer.page.evaluate(() => performance.now())
    process.kill(relayAPid, 'SIGINT')
    await viewer.page.waitForTimeout(AFTER_SWITCH_MS)
    const endedAt = await viewer.page.evaluate(() => performance.now())

    // Assert: frames keep coming at the steady pace across the switch
    const before = await presentedFrameWindow(viewer.page, steadyFrom, stoppedAt)
    const across = await presentedFrameWindow(viewer.page, stoppedAt, endedAt)
    console.log(JSON.stringify({ before, across }))
    expect(across.maxGapMs).toBeLessThan(Math.max(250, before.maxGapMs * 2))
    expect(across.meanGapMs).toBeLessThanOrEqual(before.meanGapMs * 1.1)
    await expect(viewer.playbackStatus).toContainText('Playing')

    // Assert: relay A exits once the viewer and the publisher have left it, and frames keep coming through relay B
    await expect.poll(() => processAlive(relayAPid), { timeout: 20_000 }).toBe(false)
    const exitedAt = await viewer.page.evaluate(() => performance.now())
    await viewer.page.waitForTimeout(GROUP_MS)
    const afterExit = await presentedFrameWindow(viewer.page, exitedAt, exitedAt + GROUP_MS)
    expect(afterExit.frames).toBeGreaterThan(0)
  } finally {
    await closeLiveViewerE2ESession(session)
  }
})
