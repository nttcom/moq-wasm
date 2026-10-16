import { execFileSync, spawn, spawnSync } from 'node:child_process'
import { tmpdir } from 'node:os'
import { join, resolve } from 'node:path'
import { liveViewerE2EConfig } from './live-viewer-e2e-arrange'

const REPO_ROOT = resolve(__dirname, '..', '..', '..')
/// The first run compiles the recorder unless CI built it beforehand.
const RECORDER_START_TIMEOUT_MS = 180_000

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
  return {
    codecs: info.streams.map((stream) => stream.codec_name),
    durationSeconds: Number(info.format.duration),
    largestGapSeconds: { video: largestPacketGap(path, 'v'), audio: largestPacketGap(path, 'a') },
    decodeErrors: decoded.stderr
  }
}

function largestPacketGap(path: string, stream: 'v' | 'a'): number {
  const times = execFileSync(
    'ffprobe',
    ['-v', 'error', '-select_streams', stream, '-show_entries', 'packet=pts_time', '-of', 'csv=p=0', path],
    { encoding: 'utf8' }
  )
    .trim()
    .split('\n')
    .map(Number)
    .sort((left, right) => left - right)
  return times.slice(1).reduce((largest, time, index) => Math.max(largest, time - times[index]), 0)
}
