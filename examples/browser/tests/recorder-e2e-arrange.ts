import { expect, test as base } from '@playwright/test'
import { execFileSync, spawn, spawnSync } from 'node:child_process'
import { cpSync, mkdtempSync, readFileSync, readdirSync } from 'node:fs'
import { readFile } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join, resolve } from 'node:path'
import { pathToFileURL } from 'node:url'
import { setTimeout as sleep } from 'node:timers/promises'
import {
  type RecordingLayout,
  type RecordingFormat,
  type RoomRecording,
  type Recorder,
  listRecordings
} from '../examples/meeting/src/recording/recorderApi'
import { liveViewerE2EConfig } from './live-viewer-e2e-arrange'

const REPO_ROOT = resolve(__dirname, '..', '..', '..')
/// Built by the runner that starts the relay.
const RECORDER_BINARY = join(REPO_ROOT, 'target', 'debug', 'recorder')
const BOT_DISPATCHER_BINARY = join(REPO_ROOT, 'target', 'debug', 'bot-dispatcher')
const RECORDER_START_TIMEOUT_MS = 180_000
const RECORDER_SERVE_PORT = Number(process.env.RECORDER_E2E_PORT ?? 8096)
const BOT_DISPATCHER_PORT = Number(process.env.BOT_DISPATCHER_E2E_PORT ?? 8098)
/// The recorder bot learns of a stop from its heartbeat every 5 s, then waits up to 10 s for the participants'
/// tracks to end.
const RECORDING_FINISH_TIMEOUT_MS = 30_000
/// Compositing re-encodes the whole room after it stopped, slower than real time in a debug build.
const COMPOSITE_TIMEOUT_MS = 120_000
const POLL_INTERVAL_MS = 200

/// The fields of the recorder bot's `manifest.json` that the tests read.
export interface RecordingManifest {
  members: { namespace: string; screen_share_files: { key: string }[] }[]
}

/// Records with `--from-cache` and stops the recorder with SIGINT, as Ctrl-C would, `liveSeconds`
/// after it starts recording.
export async function recordFromCache(namespace: string, liveSeconds: number): Promise<string> {
  const output = join(tmpdir(), `moqt-recorder-e2e-${process.pid}-${Date.now()}.mp4`)
  const recorder = spawn(
    RECORDER_BINARY,
    [
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

/// On failure, the store is copied into the test output directory, which CI uploads.
export const test = base.extend<{ recorderServe: RecorderServe }>({
  recorderServe: async ({}, use, testInfo) => {
    const recorder = await startRecorderServe()
    await use(recorder)
    await recorder.close()
    if (testInfo.status !== testInfo.expectedStatus) {
      cpSync(recorder.storeDir, testInfo.outputPath('recorder-store'), { recursive: true })
    }
  }
})

/// A member's archive (`<member>/camera/…json` in a MoQ room recording). Chunk paths are relative to the filesystem
/// root.
export interface MemberArchive {
  tracks: { name: string; chunks: { data: string; index: string }[] }[]
}

/// The payload of every Normal object an archive holds of `track`, in the order they were recorded; the End of Group
/// markers are left out. Throws when the archive has no such track.
export async function archivedPayloads(archive: MemberArchive, track: string): Promise<string[]> {
  const archived = archive.tracks.find((candidate) => candidate.name === track)
  if (!archived) {
    throw new Error(`the archive has no ${track} track: ${archive.tracks.map((candidate) => candidate.name)}`)
  }
  const payloads = []
  for (const chunk of archived.chunks) {
    const data = await readFile(join('/', chunk.data))
    const entries = JSON.parse(await readFile(join('/', chunk.index), 'utf8'))
    for (const entry of entries) {
      if (entry.type === 'object' && entry.status === 0) {
        payloads.push(data.subarray(Number(entry.offset), Number(entry.offset) + Number(entry.length)).toString())
      }
    }
  }
  return payloads
}

export function recordingPlayerPath(recorderUrl: string, id: string): string {
  const params = new URLSearchParams({ recorder: recorderUrl, relayUrl: liveViewerE2EConfig.moqtUrl, id })
  return `/moq-wasm/examples/recording-player/?${params}`
}

/// The fields of `bot-dispatcher`'s `GET /bots/recorder/{id}` that the fixture reads.
interface BotView {
  state: string
  error?: string
  output?: { recording_id?: string }
}

type RecorderServe = Awaited<ReturnType<typeof startRecorderServe>>

/// `recorder serve` over a directory store, and `bot-dispatcher` without authorization, which the fixture calls as an app
/// server would to start each recording as a `recorder bot` child process.
async function startRecorderServe() {
  const url = `http://127.0.0.1:${RECORDER_SERVE_PORT}`
  const dispatcherUrl = `http://127.0.0.1:${BOT_DISPATCHER_PORT}`
  const dir = mkdtempSync(join(tmpdir(), 'moqt-recorder-serve-e2e-'))
  const storeDir = join(dir, 'store')
  const storeUrl = pathToFileURL(storeDir).href + '/'
  const botTypes = JSON.stringify({
    recorder: {
      launcher: { kind: 'local', program: RECORDER_BINARY },
      args: [
        'bot',
        '--relay',
        liveViewerE2EConfig.moqtUrl,
        '--insecure-skip-tls-verify',
        '--store',
        storeUrl,
        '--output-dir',
        join(dir, 'work')
      ],
      grace_seconds: 300,
      max_seconds: 7200,
      max_running: 8
    }
  })
  // The jobs inherit the environment; GStreamer's ERROR lines name the element behind a failed composite.
  const env = { GST_DEBUG: '*:1', ...process.env, RUST_LOG: 'info' }
  const recorder = spawn(
    RECORDER_BINARY,
    [
      'serve',
      '--listen',
      `127.0.0.1:${RECORDER_SERVE_PORT}`,
      '--allowed-origin',
      'http://127.0.0.1:4173',
      '--relay',
      liveViewerE2EConfig.moqtUrl,
      '--insecure-skip-tls-verify',
      '--store',
      storeUrl,
      '--insecure-skip-auth'
    ],
    { cwd: REPO_ROOT, env, stdio: 'inherit' }
  )
  const dispatcher = spawn(BOT_DISPATCHER_BINARY, [], {
    cwd: REPO_ROOT,
    env: {
      ...env,
      PORT: String(BOT_DISPATCHER_PORT),
      BOT_DISPATCHER_PUBLIC_URL: dispatcherUrl,
      BOT_DISPATCHER_TYPES: botTypes,
      BOT_DISPATCHER_STORE_URL: pathToFileURL(join(dir, 'bot-dispatcher')).href + '/',
      BOT_DISPATCHER_INSECURE_SKIP_AUTH: 'true'
    },
    stdio: 'inherit'
  })
  const exited = Promise.all(
    [recorder, dispatcher].map((child) => new Promise<number | null>((resolveExit) => child.on('exit', resolveExit)))
  )
  const deadline = Date.now() + RECORDER_START_TIMEOUT_MS
  for (const [child, healthz] of [
    [recorder, `${url}/healthz`],
    [dispatcher, `${dispatcherUrl}/healthz`]
  ] as const) {
    while (
      !(await fetch(healthz)
        .then((response) => response.ok)
        .catch(() => false))
    ) {
      if (child.exitCode !== null || Date.now() > deadline) {
        throw new Error(`${healthz} did not answer (exit code ${child.exitCode})`)
      }
      await sleep(POLL_INTERVAL_MS)
    }
  }
  // The recorder runs with --insecure-skip-auth against a relay without authorization.
  const api: Recorder = { url, token: () => undefined }
  const current = async (recording: RoomRecording) =>
    (await listRecordings(api, recording.namespace_prefix)).find((candidate) => candidate.id === recording.id)
  /// A recording's files are under its app, the first element of its room.
  const recordingDir = (recording: RoomRecording) =>
    join(storeDir, recording.namespace_prefix.split('/')[0], recording.id)
  /// The bot id of each recording, for `stop`.
  const botIds = new Map<string, string>()
  return {
    url,
    storeDir,
    recordingDir,
    /// Returns once the recorder bot has joined the room, so that the recording covers what follows.
    async start(
      namespacePrefix: string,
      layout: RecordingLayout,
      composite = false,
      format: RecordingFormat = 'mp4'
    ): Promise<RoomRecording> {
      const bot = await jsonRequest<{ id: string }>(`${dispatcherUrl}/bots/recorder`, {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ namespace_prefix: namespacePrefix, options: { layout, composite, format } })
      })
      const deadline = Date.now() + RECORDER_START_TIMEOUT_MS
      for (;;) {
        const reported = await jsonRequest<BotView>(`${dispatcherUrl}/bots/recorder/${bot.id}`, {})
        if (reported.state === 'failed' || reported.state === 'finished') {
          throw new Error(
            `the recorder bot ${bot.id} ${reported.state} before recording: ${reported.error ?? 'no error'}`
          )
        }
        const id = reported.output?.recording_id
        const recording =
          id === undefined
            ? undefined
            : (await listRecordings(api, namespacePrefix)).find(
                (candidate) => candidate.id === id && candidate.state === 'recording'
              )
        if (recording) {
          botIds.set(recording.id, bot.id)
          return recording
        }
        if (Date.now() > deadline) {
          throw new Error(`the recorder bot ${bot.id} did not start recording`)
        }
        await sleep(POLL_INTERVAL_MS)
      }
    },
    /// The stop request returns before the MP4 files are closed; wait until the recording reports `finished`.
    async stop(recording: RoomRecording): Promise<string[]> {
      await jsonRequest(`${dispatcherUrl}/bots/recorder/${botIds.get(recording.id)}`, { method: 'DELETE' })
      const deadline = Date.now() + RECORDING_FINISH_TIMEOUT_MS
      for (;;) {
        const state = (await current(recording))?.state
        if (state === 'finished') {
          break
        }
        if (state === 'failed' || Date.now() > deadline) {
          throw new Error(`recording ${recording.id} did not finish: ${JSON.stringify(await current(recording))}`)
        }
        await sleep(POLL_INTERVAL_MS)
      }
      const dir = recordingDir(recording)
      return readdirSync(dir, { recursive: true, encoding: 'utf8' })
        .filter((name) => name.endsWith('.mp4') && name !== 'composite.mp4')
        .map((name) => join(dir, name))
    },
    /// Throws until the recording is `finished`: the recorder bot writes `manifest.json` only then.
    manifest(recording: RoomRecording): RecordingManifest {
      return JSON.parse(readFileSync(join(recordingDir(recording), 'manifest.json'), 'utf8')) as RecordingManifest
    },
    /// Waits for a stopped recording's composite and returns its path.
    async composite(recording: RoomRecording): Promise<string> {
      const deadline = Date.now() + COMPOSITE_TIMEOUT_MS
      for (;;) {
        const state = (await current(recording))?.composite?.state
        if (state === 'done') {
          return join(recordingDir(recording), 'composite.mp4')
        }
        if (state === 'failed' || state === 'skipped' || Date.now() > deadline) {
          throw new Error(`recording ${recording.id} was not composited: ${JSON.stringify(await current(recording))}`)
        }
        await sleep(POLL_INTERVAL_MS)
      }
    },
    /// Opens a replay of the recording, which runs until `close`, and returns the first line of the answer.
    async publish(recording: RoomRecording): Promise<{ answer: Record<string, unknown>; close: () => Promise<void> }> {
      const response = await fetch(`${url}/recordings/${recording.id}/publish`, { method: 'POST' })
      if (!response.ok || !response.body) {
        throw new Error(`publish answered ${response.status}: ${await response.text()}`)
      }
      const reader = response.body.getReader()
      const decoder = new TextDecoder()
      let text = ''
      while (!text.includes('\n')) {
        const { value, done } = await reader.read()
        if (done) {
          throw new Error(`the publish answer ended before its first line: ${text}`)
        }
        text += decoder.decode(value, { stream: true })
      }
      return { answer: JSON.parse(text.slice(0, text.indexOf('\n'))), close: () => reader.cancel() }
    },
    async close(): Promise<void> {
      recorder.kill('SIGINT')
      dispatcher.kill('SIGINT')
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

/// The mean red, green and blue of a box in the frame `secondsFromEnd` before the end of a video. The box edges
/// are fractions of the frame width and height.
export function meanColor(
  path: string,
  secondsFromEnd: number,
  [left, top, right, bottom]: [number, number, number, number]
): [number, number, number] {
  const crop = `crop=iw*${right - left}:ih*${bottom - top}:iw*${left}:ih*${top}`
  const [red, green, blue] = execFileSync('ffmpeg', [
    '-v',
    'error',
    '-sseof',
    `-${secondsFromEnd}`,
    '-i',
    path,
    '-frames:v',
    '1',
    '-vf',
    `${crop},scale=1:1:flags=area`,
    '-f',
    'rawvideo',
    '-pix_fmt',
    'rgb24',
    '-'
  ])
  return [red, green, blue]
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

async function jsonRequest<T>(url: string, init: RequestInit): Promise<T> {
  const response = await fetch(url, init)
  const body = await response.json().catch(() => ({}))
  if (!response.ok) {
    throw new Error(body.error ?? `${url} answered ${response.status}`)
  }
  return body as T
}
