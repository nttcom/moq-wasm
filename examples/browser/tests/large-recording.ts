import { createHash } from 'node:crypto'
import { link, mkdir, readFile, writeFile } from 'node:fs/promises'
import { dirname, join } from 'node:path'

// Share payload files while shifting object IDs and clocks to test large logical recordings.
export async function createLargeRecording(
  storeDir: string,
  originalId: string
): Promise<{ id: string; bytes: number; periodSeconds: number }> {
  const id = `${originalId}-large`
  const prefix = join(storeDir, id).replace(/^\//, '')
  const room = JSON.parse(await readFile(join(storeDir, originalId, 'manifest.json'), 'utf8'))
  const member = room.members.find((member: { camera: string[] }) => member.camera.length)
  const original = JSON.parse(await readFile(join('/', member.camera[0]), 'utf8'))
  const media = original.tracks.filter((track: { name: string }) => track.name === 'video' || track.name === 'audio')
  const bytes = media.reduce(
    (total: number, track: any) =>
      total + track.chunks.reduce((sum: number, chunk: any) => sum + Number(chunk.data_size), 0),
    0
  )
  if (!bytes) throw new Error('Fixture has no media bytes')
  const repeats = Math.ceil((257 * 1024 * 1024) / bytes)
  const times = media.flatMap((track: any) =>
    track.chunks.flatMap((chunk: any) =>
      [chunk.capture_start_us, chunk.capture_end_us].filter((value: unknown) => value !== null).map(Number)
    )
  )
  const period = BigInt(Math.max(...times) - Math.min(...times) + 1_000_000)
  const tracks = []
  for (const track of original.tracks.filter((track: { name: string }) =>
    ['catalog', 'video', 'audio'].includes(track.name)
  )) {
    const chunks = []
    const sourceChunks = []
    for (let index = 0; index < track.chunks.length; index++) {
      const chunk = track.chunks[index]
      const dataKey = `${prefix}/archive/tracks/${track.id}/source-${index}.data`
      await mkdir(dirname(join('/', dataKey)), { recursive: true })
      await link(join('/', chunk.data), join('/', dataKey))
      sourceChunks.push({ chunk, dataKey, entries: JSON.parse(await readFile(join('/', chunk.index), 'utf8')) })
    }
    for (let repeat = 0; repeat < (track.name === 'catalog' ? 1 : repeats); repeat++) {
      const timeOffset = BigInt(repeat) * period
      const groupOffset = BigInt(repeat) * 1_000_000n
      for (const { chunk, dataKey, entries: originals } of sourceChunks) {
        const entries = originals.map((original: any) => {
          const entry = { ...original, group: String(BigInt(original.group) + groupOffset) }
          if (entry.capture_us !== undefined && entry.capture_us !== null) {
            entry.capture_us = String(BigInt(entry.capture_us) + timeOffset)
            entry.extensions = shiftCaptureTimestamp(entry.extensions, timeOffset)
          }
          return entry
        })
        const index = Buffer.from(JSON.stringify(entries))
        const indexKey = `${prefix}/archive/tracks/${track.id}/${chunks.length}.index.json`
        await writeFile(join('/', indexKey), index)
        const shiftLocation = (value: any) =>
          value === null ? null : { ...value, group: String(BigInt(value.group) + groupOffset) }
        const shiftTime = (value: string | null) => (value === null ? null : String(BigInt(value) + timeOffset))
        chunks.push({
          ...chunk,
          data: dataKey,
          index: indexKey,
          index_size: String(index.length),
          index_sha256: createHash('sha256').update(index).digest('hex'),
          first: shiftLocation(chunk.first),
          last: shiftLocation(chunk.last),
          capture_start_us: shiftTime(chunk.capture_start_us),
          capture_end_us: shiftTime(chunk.capture_end_us)
        })
      }
    }
    tracks.push({ ...track, chunks })
  }
  const archiveKey = `${prefix}/archive/manifest.json`
  await writeFile(join('/', archiveKey), JSON.stringify({ ...original, tracks }))
  await writeFile(
    join(storeDir, id, 'manifest.json'),
    JSON.stringify({
      version: 1,
      format: 'moq',
      members: [{ namespace: member.namespace, camera: [archiveKey], screen_share: [] }]
    })
  )
  const state = JSON.parse(await readFile(join(storeDir, 'state', `${originalId}.json`), 'utf8'))
  await writeFile(join(storeDir, 'state', `${id}.json`), JSON.stringify({ ...state, id }))
  return { id, bytes: bytes * repeats, periodSeconds: Number(period) / 1_000_000 }
}

function shiftCaptureTimestamp(encoded: string, offset: bigint): string {
  const data = Buffer.from(encoded, 'base64')
  function read(at: number): { value: bigint; next: number; length: number } {
    const length = 1 << (data[at] >> 6)
    if (at + length > data.length) throw new Error('Truncated fixture extension')
    let value = BigInt(data[at] & 63)
    for (let byte = 1; byte < length; byte++) value = (value << 8n) | BigInt(data[at + byte])
    return { value, next: at + length, length }
  }
  let position = read(0).next
  let found = false
  while (position < data.length) {
    const key = read(position)
    const value = read(key.next)
    if (key.value === 16n) {
      if (value.length !== 8) throw new Error('Fixture capture time must be a Unix microsecond varint')
      data.writeBigUInt64BE((value.value + offset) | (3n << 62n), key.next)
      found = true
    }
    position = value.next + (key.value % 2n ? Number(value.value) : 0)
  }
  if (!found) throw new Error('Fixture has no capture timestamp extension')
  return data.toString('base64')
}
