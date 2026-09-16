import { readFileSync } from 'node:fs'
import { expect, test } from '@playwright/test'
import { LIVE_VIEWER_PATH } from '../playwright.helpers'

const moqtUrl = process.env.MEDIA_E2E_MOQT_URL ?? 'https://127.0.0.1:4433'
const namespace = process.env.LIVE_VIEWER_E2E_NAMESPACE ?? 'anon/live/e2e'
const bridgeLog = process.env.DELIVERY_BRIDGE_LOG
const watchSeconds = Number(process.env.DELIVERY_SECONDS ?? '30')

type Track = 'video' | 'audio'

type Received = {
  track: Track
  groupId: string
  captureMicros: number
  bytes: number
}

type Published = {
  stage: 'ingest' | 'publish'
  track: Track
  groupId?: string
  captureMicros?: number
  ptsMicros: number
  bytes: number
}

type TrackReport = {
  received: number
  published: number
  missing: Published[]
  unmatched: Received[]
  ingestedButNotPublished: number
}

/// The viewer hands every object it receives to a decoder worker, so the
/// objects are recorded where they cross to the worker: the first worker the
/// page creates decodes video, the second audio.
test('every sample the bridge publishes while the viewer watches reaches it', async ({ browser }) => {
  test.skip(
    !bridgeLog,
    'set DELIVERY_BRIDGE_LOG to the live-ingest log written with RUST_LOG=moqt_bridge_live_ingest::delivery=debug'
  )
  // Arrange
  const context = await browser.newContext({ ignoreHTTPSErrors: true })
  await context.addInitScript(() => {
    const received: Received[] = []
    let workers = 0
    ;(window as any).__delivery = received
    const OriginalWorker = window.Worker
    ;(window as any).Worker = class extends OriginalWorker {
      constructor(url: string | URL, options?: WorkerOptions) {
        super(url, options)
        ;(this as any).__track = ++workers === 1 ? 'video' : 'audio'
      }
    }
    const originalPost = OriginalWorker.prototype.postMessage
    OriginalWorker.prototype.postMessage = function (message: any, ...rest: any[]) {
      const object = message?.subgroupStreamObject
      if (message?.groupId !== undefined && object && object.objectStatus == null && object.objectPayloadLength > 0) {
        const capture = (object.locHeader as { id: number; value: { varint?: number } }[] | undefined)?.find(
          (extension) => extension.id === 2
        )
        received.push({
          track: (this as any).__track,
          groupId: String(message.groupId),
          captureMicros: capture?.value?.varint,
          bytes: object.objectPayloadLength
        })
      }
      return (originalPost as any).apply(this, [message, ...rest])
    }
  })
  const page = await context.newPage()
  const params = new URLSearchParams({ moqtUrl, trackNamespace: namespace })
  await page.goto(`${LIVE_VIEWER_PATH}?${params.toString()}`, { waitUntil: 'domcontentloaded' })

  try {
    // Act
    await page.getByTestId('live-viewer-watch-button').click()
    await expect(page.getByTestId('live-viewer-playback-status')).toContainText('Playing', { timeout: 30_000 })
    await page.waitForTimeout(watchSeconds * 1000)
    const received = await page.evaluate(() => (window as any).__delivery as Received[])
    const published = parseBridgeLog(readFileSync(bridgeLog!, 'utf8'))
    const report = { video: compare('video', received, published), audio: compare('audio', received, published) }
    console.log(describe(report))

    // Assert
    expect(report.video.received).toBeGreaterThan(0)
    expect(report.audio.received).toBeGreaterThan(0)
    expect(report.video.missing).toEqual([])
    expect(report.audio.missing).toEqual([])
  } finally {
    await context.close()
  }
})

function parseBridgeLog(text: string): Published[] {
  const published: Published[] = []
  for (const raw of text.split('\n')) {
    const line = raw.replace(/\x1b\[[0-9;]*m/g, '')
    if (!line.includes('moqt_bridge_live_ingest::delivery')) {
      continue
    }
    const fields = new Map<string, string>()
    for (const match of line.matchAll(/(\w+)=("([^"]*)"|\S+)/g)) {
      fields.set(match[1], match[3] ?? match[2])
    }
    const stage = fields.get('stage')
    const track = fields.get('track')
    if (fields.get('namespace') !== namespace) {
      continue
    }
    if ((stage !== 'ingest' && stage !== 'publish') || (track !== 'video' && track !== 'audio')) {
      continue
    }
    published.push({
      stage,
      track,
      groupId: fields.get('group_id'),
      captureMicros: fields.has('capture_us') ? Number(fields.get('capture_us')) : undefined,
      ptsMicros: Number(fields.get('pts_us')),
      bytes: Number(fields.get('bytes'))
    })
  }
  return published
}

/// Only the span the viewer actually watched is compared: from the first to
/// the last capture timestamp it received on the track.
function compare(track: Track, received: Received[], published: Published[]): TrackReport {
  const seen = received.filter((sample) => sample.track === track && sample.captureMicros !== undefined)
  const captures = new Set(seen.map((sample) => sample.captureMicros))
  const first = Math.min(...captures)
  const last = Math.max(...captures)
  const inSpan = (captureMicros: number | undefined) =>
    captureMicros !== undefined && captureMicros >= first && captureMicros <= last
  const publishedInSpan = published.filter(
    (sample) => sample.track === track && sample.stage === 'publish' && inSpan(sample.captureMicros)
  )
  const publishedCaptures = new Set(publishedInSpan.map((sample) => sample.captureMicros))
  const publishedPts = new Set(publishedInSpan.map((sample) => sample.ptsMicros))
  const ptsSpan = publishedInSpan.map((sample) => sample.ptsMicros)
  const firstPts = Math.min(...ptsSpan)
  const lastPts = Math.max(...ptsSpan)
  const ingestedButNotPublished = published.filter(
    (sample) =>
      sample.track === track &&
      sample.stage === 'ingest' &&
      sample.ptsMicros >= firstPts &&
      sample.ptsMicros <= lastPts &&
      !publishedPts.has(sample.ptsMicros)
  ).length
  return {
    received: seen.length,
    published: publishedInSpan.length,
    missing: publishedInSpan.filter((sample) => !captures.has(sample.captureMicros!)),
    unmatched: seen.filter((sample) => !publishedCaptures.has(sample.captureMicros)),
    ingestedButNotPublished
  }
}

function describe(report: Record<Track, TrackReport>): string {
  const lines = [`delivery of ${namespace} over ${watchSeconds}s`]
  for (const track of ['video', 'audio'] as Track[]) {
    const { received, published, missing, unmatched, ingestedButNotPublished } = report[track]
    lines.push(
      `${track}: published ${published}, received ${received}, missing ${missing.length}, unmatched ${unmatched.length}, ingested but not published ${ingestedButNotPublished}`
    )
    for (const sample of missing.slice(0, 10)) {
      lines.push(
        `  missing ${track} group ${sample.groupId} capture ${sample.captureMicros} pts ${sample.ptsMicros} ${sample.bytes} bytes`
      )
    }
  }
  return lines.join('\n')
}
