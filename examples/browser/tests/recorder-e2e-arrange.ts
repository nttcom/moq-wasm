import { expect } from '@playwright/test'
import { execFileSync, spawn, spawnSync } from 'node:child_process'
import { mkdtempSync, readdirSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join, resolve } from 'node:path'
import { setTimeout as sleep } from 'node:timers/promises'
import {
  type RecordingLayout,
  type RoomRecording,
  listRecordings,
  startRecording,
  stopRecording
} from '../examples/meeting/src/recording/recorderApi'
import { liveViewerE2EConfig } from './live-viewer-e2e-arrange'

const REPO_ROOT = resolve(__dirname, '..', '..', '..')
/// The first run compiles the recorder unless CI built it beforehand.
const RECORDER_START_TIMEOUT_MS = 180_000
const RECORDER_SERVE_PORT = Number(process.env.RECORDER_E2E_PORT ?? 8096)
const RECORDING_FINISH_TIMEOUT_MS = 15_000
/// Compositing re-encodes the whole room after it stopped, slower than real time in a debug build.
const COMPOSITE_TIMEOUT_MS = 120_000
const POLL_INTERVAL_MS = 200

/// Records with `--from-cache` and stops the recorder with SIGINT, as Ctrl-C would, `liveSeconds`
/// after it starts recording.
export async function recordFromCache(namespace: string, liveSeconds: number): Promise<string> {
  const output = join(tmpdir(), `moqt-recorder-e2e-${process.pid}-${Date.now()}.mp4`)
  const recorder = spawn(
    'cargo',
    [
      'run',
      '-q',
      '-p',
      'recorder',
      '--',
      '--relay',
      liveViewerE2EConfig.moqtUrl,
      '--insecure-skip-tls-verify',
      '--namespace',
      namespace,
      '--output',
      output,
      '--from-cache'
    ],
    { cwd: REPO_ROOT, env: { ...process.env, RUST_LOG: 'info' }, stdio: ['ignore', 'inherit', 'pipe'] }
  )
  const exited = new Promise<number | null>((resolveExit) => recorder.on('exit', resolveExit))
  await new Promise<void>((resolveStarted, rejectStarted) => {
    const timer = setTimeout(
      () => rejectStarted(new Error('the recorder did not start recording')),
      RECORDER_START_TIMEOUT_MS
    )
    recorder.stderr.on('data', (chunk: Buffer) => {
      process.stderr.write(chunk)
      if (chunk.toString().includes('recording started')) {
        clearTimeout(timer)
        resolveStarted()
      }
    })
    void exited.then((code) => rejectStarted(new Error(`the recorder exited with ${code} before recording`)))
  })
  await new Promise((resolveWait) => setTimeout(resolveWait, liveSeconds * 1_000))
  recorder.kill('SIGINT')
  const code = await exited
  if (code !== 0) {
    throw new Error(`the recorder exited with ${code}`)
  }
  return output
}

export async function startRecorderServe() {
  const url = `http://127.0.0.1:${RECORDER_SERVE_PORT}`
  const outputDir = mkdtempSync(join(tmpdir(), 'moqt-recorder-serve-e2e-'))
  const recorder = spawn(
    'cargo',
    [
      'run',
      '-q',
      '-p',
      'recorder',
      '--',
      'serve',
      '--listen',
      `127.0.0.1:${RECORDER_SERVE_PORT}`,
      '--relay',
      liveViewerE2EConfig.moqtUrl,
      '--insecure-skip-tls-verify',
      '--output-dir',
      outputDir,
      '--composite'
    ],
    { cwd: REPO_ROOT, env: { ...process.env, RUST_LOG: 'info' }, stdio: 'inherit' }
  )
  const exited = new Promise<number | null>((resolveExit) => recorder.on('exit', resolveExit))
  const deadline = Date.now() + RECORDER_START_TIMEOUT_MS
  while (
    !(await fetch(`${url}/healthz`)
      .then((response) => response.ok)
      .catch(() => false))
  ) {
    if (recorder.exitCode !== null || Date.now() > deadline) {
      throw new Error(`recorder serve did not start (exit code ${recorder.exitCode})`)
    }
    await sleep(POLL_INTERVAL_MS)
  }
  return {
    start: (namespacePrefix: string, layout: RecordingLayout, composite = false) =>
      startRecording(url, namespacePrefix, layout, composite),
    /// The stop request returns before the MP4 files are closed; wait until the recording reports `finished`.
    async stop(recording: RoomRecording): Promise<string[]> {
      await stopRecording(url, recording.id)
      const deadline = Date.now() + RECORDING_FINISH_TIMEOUT_MS
      for (;;) {
        const current = (await listRecordings(url, recording.namespace_prefix)).find(
          (candidate) => candidate.id === recording.id
        )
        if (current?.state === 'finished') {
          break
        }
        if (current?.state === 'failed' || Date.now() > deadline) {
          throw new Error(`recording ${recording.id} did not finish: ${JSON.stringify(current)}`)
        }
        await sleep(POLL_INTERVAL_MS)
      }
      const dir = join(outputDir, recording.id)
      return readdirSync(dir, { recursive: true, encoding: 'utf8' })
        .filter((name) => name.endsWith('.mp4'))
        .map((name) => join(dir, name))
    },
    /// Waits for a stopped recording's composite and returns its path.
    async composite(recording: RoomRecording): Promise<string> {
      const deadline = Date.now() + COMPOSITE_TIMEOUT_MS
      for (;;) {
        const current = (await listRecordings(url, recording.namespace_prefix)).find(
          (candidate) => candidate.id === recording.id
        )
        const state = current?.composite?.state
        if (state === 'done') {
          return join(outputDir, recording.id, 'composite.mp4')
        }
        if (state === 'failed' || state === 'skipped' || Date.now() > deadline) {
          throw new Error(`recording ${recording.id} was not composited: ${JSON.stringify(current)}`)
        }
        await sleep(POLL_INTERVAL_MS)
      }
    },
    async close(): Promise<void> {
      recorder.kill('SIGINT')
      await exited
    }
  }
}

export function probeRecording(path: string) {
  const info = JSON.parse(
    execFileSync(
      'ffprobe',
      ['-v', 'error', '-show_entries', 'stream=codec_name:format=duration', '-of', 'json', path],
      { encoding: 'utf8' }
    )
  ) as { streams: { codec_name: string }[]; format: { duration: string } }
  // ffprobe decodes every frame without encoding it again; an ffmpeg null output would also report its encoder and
  // muxer complaints about timestamps, such as across the gap a muted microphone leaves.
  const decoded = spawnSync('ffprobe', ['-v', 'error', '-show_entries', 'frame=key_frame', '-of', 'csv=p=0', path], {
    encoding: 'utf8'
  })
  const packetSeconds = { video: packetTimes(path, 'v'), audio: packetTimes(path, 'a') }
  return {
    codecs: info.streams.map((stream) => stream.codec_name),
    durationSeconds: Number(info.format.duration),
    packetSeconds,
    largestGapSeconds: { video: largestGap(packetSeconds.video), audio: largestGap(packetSeconds.audio) },
    decodeErrors: decoded.stderr
  }
}

/// The probe with the most packets of `track`.
export function longestRecording(probes: ReturnType<typeof probeRecording>[], track: 'video' | 'audio') {
  const [file] = [...probes].sort((left, right) => right.packetSeconds[track].length - left.packetSeconds[track].length)
  expect(file, `a file with ${track}`).toBeDefined()
  return file
}

/// `times` must be in ascending order.
export function splitAtGaps(times: number[], minGapSeconds: number): number[][] {
  const runs: number[][] = []
  for (const [index, time] of times.entries()) {
    if (index === 0 || time - times[index - 1] > minGapSeconds) {
      runs.push([])
    }
    runs[runs.length - 1].push(time)
  }
  return runs
}

function packetTimes(path: string, stream: 'v' | 'a'): number[] {
  // A file without the stream prints nothing, and `Number('')` would turn that into a packet at 0 s.
  return execFileSync(
    'ffprobe',
    ['-v', 'error', '-select_streams', stream, '-show_entries', 'packet=pts_time', '-of', 'csv=p=0', path],
    { encoding: 'utf8' }
  )
    .split('\n')
    .filter((line) => line.trim() !== '')
    .map(Number)
    .sort((left, right) => left - right)
}

function largestGap(times: number[]): number {
  return times.slice(1).reduce((largest, time, index) => Math.max(largest, time - times[index]), 0)
}
