import { expect, test } from '@playwright/test'
import { buildMsfCatalogJson, extractCatalogAudioTracks, extractCatalogVideoTracks } from './catalog'

const AUDIO_SPECIFIC_CONFIG = 'ERA='

let warnings: unknown[][]
let originalWarn: typeof console.warn

test.beforeEach(() => {
  warnings = []
  originalWarn = console.warn
  console.warn = (...args: unknown[]) => warnings.push(args)
})

test.afterEach(() => {
  console.warn = originalWarn
})

function catalogWithInitDataList(initDataList: unknown[]): unknown {
  return {
    version: 1,
    tracks: [
      { name: 'video', packaging: 'loc', role: 'video', isLive: true, codec: 'avc1.640028' },
      { name: 'audio', packaging: 'loc', role: 'audio', isLive: true, codec: 'mp4a.40.2', initRef: 'audio' }
    ],
    initDataList
  }
}

test('a track takes the data of the inline entry its initRef names', () => {
  // Arrange
  const catalog = catalogWithInitDataList([
    { id: 'video_cmaf', type: 'inline', data: 'AAAA' },
    { id: 'audio', type: 'inline', data: AUDIO_SPECIFIC_CONFIG }
  ])

  // Act
  const [video] = extractCatalogVideoTracks(catalog)
  const [audio] = extractCatalogAudioTracks(catalog)

  // Assert: a track without initRef has no init data and is not warned about
  expect(audio.initData).toBe(AUDIO_SPECIFIC_CONFIG)
  expect(video.initData).toBeUndefined()
  expect(warnings).toEqual([])
})

for (const [reference, initDataList] of [
  ['names no entry', [{ id: 'video_cmaf', type: 'inline', data: 'AAAA' }]],
  ['names an entry of a type the draft does not define', [{ id: 'audio', type: 'url', data: 'https://example.com/a' }]]
] as const) {
  test(`an initRef that ${reference} leaves the track without init data and warns`, () => {
    // Arrange
    const catalog = catalogWithInitDataList([...initDataList])

    // Act
    const [audio] = extractCatalogAudioTracks(catalog)

    // Assert
    expect(audio.initData).toBeUndefined()
    expect(warnings).toHaveLength(1)
  })
}

test('the written catalog puts initDataList after tracks and leaves an empty one out', () => {
  // Arrange
  const audio = { name: 'audio', packaging: 'loc', isLive: true, initRef: 'audio' }
  const video = { name: 'video', packaging: 'loc', isLive: true }

  // Act
  const withInitData = buildMsfCatalogJson([audio], [{ id: 'audio', type: 'inline', data: AUDIO_SPECIFIC_CONFIG }])
  const withoutInitData = buildMsfCatalogJson([video])

  // Assert
  expect(Object.keys(JSON.parse(withInitData))).toEqual(['version', 'generatedAt', 'tracks', 'initDataList'])
  expect(Object.keys(JSON.parse(withoutInitData))).toEqual(['version', 'generatedAt', 'tracks'])
})
