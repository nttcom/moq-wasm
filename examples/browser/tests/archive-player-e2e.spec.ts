import { expect, test } from '@playwright/test'
import { spawn } from 'node:child_process'
import { mkdir, mkdtemp, readFile, rm, writeFile } from 'node:fs/promises'
import { createHash } from 'node:crypto'
import { pathToFileURL } from 'node:url'
import { tmpdir } from 'node:os'
import { join, resolve } from 'node:path'
import { setTimeout as sleep } from 'node:timers/promises'

test.setTimeout(160_000)

test('records a live broadcast and plays and seeks its archive', async ({ page }) => {
  // Arrange: the live-viewer E2E runner starts the relay and a test broadcast
  const dir = await mkdtemp(join(tmpdir(), 'moq-archive-e2e-'))
  const path = join(dir, 'recording.jsonl')
  const store = join(dir, 'store')
  await mkdir(store)
  const recorder = spawn(
    resolve(process.cwd(), '../../target/debug/recorder'),
    [
      '--relay',
      process.env.MEDIA_E2E_MOQT_URL ?? 'https://127.0.0.1:4433',
      '--namespace',
      process.env.LIVE_VIEWER_E2E_NAMESPACE ?? 'anon/live/e2e',
      '--insecure-skip-tls-verify',
      '--archive',
      '--upload',
      pathToFileURL(`${store}/`).href,
      '--output',
      path
    ],
    { cwd: resolve(process.cwd(), '../..'), stdio: ['ignore', 'pipe', 'pipe'] }
  )
  let logs = ''
  recorder.stdout.on('data', (chunk) => {
    logs += chunk.toString()
  })
  recorder.stderr.on('data', (chunk) => {
    logs += chunk.toString()
  })
  const exited = new Promise<number | null>((resolveExit) => recorder.once('exit', resolveExit))

  try {
    // Act: record enough timestamped video to seek, then close the archive
    await expect
      .poll(
        async () => {
          if (recorder.exitCode !== null) throw new Error(`recorder exited early: ${logs}`)
          const text = await readFile(path, 'utf8').catch(() => '')
          const times = text
            .slice(0, text.lastIndexOf('\n') + 1)
            .split('\n')
            .filter(Boolean)
            .map((line) => JSON.parse(line))
            .filter((entry) => entry.track !== 'catalog' && entry.capture_us)
            .map((entry) => Number(entry.capture_us))
          return times.length ? (Math.max(...times) - Math.min(...times)) / 1_000_000 : 0
        },
        { timeout: 100_000 }
      )
      .toBeGreaterThan(36)
    recorder.kill('SIGINT')
    expect(await exited, logs).toBe(0)
    const namespace = process.env.LIVE_VIEWER_E2E_NAMESPACE ?? 'anon/live/e2e'
    const manifest = JSON.parse(await readFile(join(store, namespace, 'recording.jsonl', 'manifest.json'), 'utf8'))
    expect(manifest.tracks.length).toBeGreaterThanOrEqual(3)
    for (const track of manifest.tracks) {
      for (const chunk of track.chunks) {
        const data = await readFile(join('/', chunk.data))
        const index = await readFile(join('/', chunk.index))
        expect(createHash('sha256').update(data).digest('hex')).toBe(chunk.data_sha256)
        expect(createHash('sha256').update(index).digest('hex')).toBe(chunk.index_sha256)
      }
    }
    const saved = (await readFile(path, 'utf8'))
      .split('\n')
      .filter(Boolean)
      .map((line) => JSON.parse(line))
    for (const track of manifest.tracks) {
      const expected = saved.filter((entry) => entry.track === track.name && entry.group && entry.object !== undefined)
      const restored = []
      for (const chunk of track.chunks) {
        const data = await readFile(join('/', chunk.data))
        const index = JSON.parse(await readFile(join('/', chunk.index), 'utf8'))
        for (const entry of index.filter((entry: { type: string }) => entry.type === 'object')) {
          restored.push({
            group: entry.group,
            object: entry.object,
            status: entry.status,
            extensions: entry.extensions,
            payload: data.subarray(Number(entry.offset), Number(entry.offset) + Number(entry.length)).toString('base64')
          })
        }
      }
      expect(restored).toEqual(
        expected.map((entry) => ({
          group: entry.group,
          object: entry.object,
          status: entry.status,
          extensions: entry.extensions,
          payload: entry.payload
        }))
      )
    }
    const tracks = saved
      .filter((entry) => entry.track === 'catalog' && entry.payload)
      .flatMap((entry) => JSON.parse(Buffer.from(entry.payload, 'base64').toString()).tracks ?? [])
    const audioNames: string[] = tracks
      .filter((track: { role?: string }) => track.role === 'audio')
      .map((track: { name: string }) => track.name)
    expect(saved.some((entry) => audioNames.includes(entry.track))).toBe(true)

    // A multibyte namespace makes byte offsets differ from string offsets.
    saved[0].namespace = 'anon/録画'
    await writeFile(path, saved.map((entry) => JSON.stringify(entry)).join('\n') + '\n')

    await page.addInitScript(() => {
      const start = AudioBufferSourceNode.prototype.start
      AudioBufferSourceNode.prototype.start = function (...args) {
        if (this.buffer && this.buffer.getChannelData(0).some((sample) => Math.abs(sample) > 0.001)) {
          document.documentElement.dataset.audioPlayed = 'true'
        }
        return start.apply(this, args)
      }
    })
    await page.goto('/moq-wasm/examples/archive-player/')
    await page.locator('#file').setInputFiles(path)
    await expect(page.locator('#status')).toHaveText('Ready')
    await page.locator('#play').click()

    // Assert: decoded pixels reach the canvas, then a seek advances the playhead
    await expect
      .poll(
        () =>
          page.locator('#video').evaluate((element: HTMLCanvasElement) => {
            const pixels = element.getContext('2d')!.getImageData(0, 0, element.width, element.height).data
            return pixels.some((value, index) => index % 4 !== 3 && value > 16)
          }),
        { timeout: 20_000 }
      )
      .toBe(true)
    await expect(page.locator('html')).toHaveAttribute('data-audio-played', 'true', { timeout: 20_000 })
    const beforeSeek = Number(await page.locator('#video').getAttribute('data-capture-us'))
    await expect
      .poll(async () => Number(await page.locator('#video').getAttribute('data-capture-us')), { timeout: 20_000 })
      .toBeGreaterThan(beforeSeek + 6_000_000)
    await page.locator('#seek').evaluate((element: HTMLInputElement) => {
      element.value = '32'
      element.dispatchEvent(new Event('change', { bubbles: true }))
    })
    await expect
      .poll(async () => Number(await page.locator('#video').getAttribute('data-capture-us')), { timeout: 15_000 })
      .toBeGreaterThan(beforeSeek + 30_000_000)
  } finally {
    if (recorder.exitCode === null) {
      recorder.kill('SIGINT')
      await exited
    }
    await rm(dir, { recursive: true, force: true })
  }
})
