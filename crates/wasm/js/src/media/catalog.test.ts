import { expect, test } from '@playwright/test'
import {
  type MediaCatalogTrack,
  buildMsfCatalogJson,
  extractCatalogTracks,
  mediaTimelineOf,
  sourceTracks
} from './catalog'

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
    version: 'draft-01',
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
  const [video] = extractCatalogTracks(catalog, 'video')
  const [audio] = extractCatalogTracks(catalog, 'audio')

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
    const [audio] = extractCatalogTracks(catalog, 'audio')

    // Assert
    expect(audio.initData).toBeUndefined()
    expect(warnings).toHaveLength(1)
  })
}

test('the written catalog names its version as the draft release, a string', () => {
  // Act
  const catalog = JSON.parse(buildMsfCatalogJson([{ name: 'video', packaging: 'loc', isLive: true }]))

  // Assert
  expect(catalog.version).toBe('draft-01')
})

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

test('a video track keeps its alternate group and bitrate', () => {
  // Arrange
  const catalog = {
    version: 'draft-01',
    tracks: [
      { name: 'video', packaging: 'loc', role: 'video', isLive: true, altGroup: 1 },
      { name: 'video_480p', packaging: 'loc', role: 'video', isLive: true, altGroup: 1, bitrate: 1_200_000 }
    ]
  }

  // Act
  const [source, rendition] = extractCatalogTracks(catalog, 'video')

  // Assert
  expect(source.altGroup).toBe(1)
  expect(source.bitrate).toBeUndefined()
  expect(rendition.altGroup).toBe(1)
  expect(rendition.bitrate).toBe(1_200_000)
})

test('a track keeps whether it is live and the duration of one that is not', () => {
  // Arrange
  const catalog = {
    version: 1,
    isComplete: true,
    tracks: [
      { name: 'video', packaging: 'loc', role: 'video', isLive: false, trackDuration: 37_250 },
      { name: 'audio', packaging: 'loc', role: 'audio', isLive: true }
    ]
  }

  // Act
  const [video, audio] = extractCatalogTracks(catalog)

  // Assert
  expect([video.isLive, video.trackDuration]).toEqual([false, 37_250])
  expect([audio.isLive, audio.trackDuration]).toEqual([true, undefined])
})

test('an event timeline keeps the type of its records', () => {
  // Arrange
  const catalog = {
    version: 1,
    tracks: [
      { name: 'video', packaging: 'loc', role: 'video', isLive: true },
      { name: 'chat', packaging: 'eventtimeline', eventType: 'com.skyway.chat.v1', isLive: true, depends: ['video'] }
    ]
  }

  // Act
  const [video, chat] = extractCatalogTracks(catalog)

  // Assert
  expect([video.eventType, chat.eventType]).toEqual([undefined, 'com.skyway.chat.v1'])
})

/// The tracks `publish()` lists for a camera with a rendition and a shared screen.
function cameraAndScreenTracks({ withoutCamera = false } = {}): MediaCatalogTrack[] {
  const track = (name: string, packaging: string, altGroup?: number): MediaCatalogTrack => ({
    name,
    label: name,
    packaging,
    altGroup
  })
  return [
    ...(withoutCamera ? [] : [track('video', 'loc', 1), track('video_cmaf', 'cmaf', 2)]),
    track('video_low', 'loc', 1),
    track('video_low_cmaf', 'cmaf', 2),
    track('audio', 'loc'),
    track('screenshare', 'loc'),
    track('screenshare_audio', 'loc')
  ]
}

test('a source keeps its video, the renditions of its alternate group, its audio and their CMAF siblings', () => {
  // Arrange
  const tracks = cameraAndScreenTracks()

  // Act
  const camera = sourceTracks(tracks, { video: 'video', audio: 'audio' })
  const screen = sourceTracks(tracks, { video: 'screenshare', audio: 'screenshare_audio' })

  // Assert
  expect(camera.map(({ name }) => name).sort()).toEqual(['audio', 'video', 'video_cmaf', 'video_low', 'video_low_cmaf'])
  expect(screen.map(({ name }) => name).sort()).toEqual(['screenshare', 'screenshare_audio'])
})

test('a source whose video is absent keeps its audio', () => {
  // Arrange: the camera is muted while its microphone is not
  const tracks = cameraAndScreenTracks({ withoutCamera: true })

  // Act
  const camera = sourceTracks(tracks, { video: 'video', audio: 'audio' })

  // Assert
  expect(camera.map(({ name }) => name)).toEqual(['audio'])
})

test('the alternates of a source audio belong to it, apart from video alternates of the same number', () => {
  // Arrange: a translated broadcast numbers its languages altGroup 1, as the video's renditions are
  const track = (name: string, role: string, altGroup?: number): MediaCatalogTrack => ({
    name,
    label: name,
    packaging: 'loc',
    role,
    altGroup
  })
  const tracks = [
    track('video', 'video', 1),
    track('video_low', 'video', 1),
    track('audio', 'audio', 1),
    track('en', 'audio', 1),
    track('commentary', 'audio')
  ]

  // Act
  const source = sourceTracks(tracks, { video: 'video', audio: 'audio' })

  // Assert
  expect(source.map(({ name }) => name)).toEqual(['video', 'video_low', 'audio', 'en'])
})

test('a player finds the media timeline of its own source', () => {
  // Arrange
  const catalog = {
    tracks: [
      { name: 'screenshare', packaging: 'loc', role: 'video', isLive: true },
      { name: 'video', packaging: 'loc', role: 'video', isLive: true },
      { name: 'audio', packaging: 'loc', role: 'audio', isLive: true },
      { name: 'screenshare_timeline', packaging: 'mediatimeline', isLive: true, depends: ['screenshare'] },
      { name: 'video_timeline', packaging: 'mediatimeline', isLive: true, depends: ['video', 'audio'] }
    ]
  }

  // Act
  const camera = mediaTimelineOf(catalog, ['video', 'audio'])
  const cameraWithoutVideo = mediaTimelineOf(catalog, ['audio'])
  const screen = mediaTimelineOf(catalog, ['screenshare'])

  // Assert
  expect([camera?.name, cameraWithoutVideo?.name, screen?.name]).toEqual([
    'video_timeline',
    'video_timeline',
    'screenshare_timeline'
  ])
})
