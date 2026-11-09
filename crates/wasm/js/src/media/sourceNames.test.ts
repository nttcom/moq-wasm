import { expect, test } from '@playwright/test'
import type { MediaCatalogTrack } from './catalog'
import { catalogSources, liveSource, rejectedName, sourceTrackNames } from './sourceNames'

function track(name: string, altGroup?: number, packaging = 'loc'): MediaCatalogTrack {
  return { name, label: name, packaging, altGroup }
}

test('a source may take any name its tracks do not share with the broadcast', () => {
  // Arrange
  const taken = new Set(['camera', 'camera_audio', 'camera_360p'])

  // Act / Assert
  expect(rejectedName('screen', sourceTrackNames('screen', ['720p']), taken)).toBeUndefined()
})

test('a name the broadcast lists or derives tracks from is rejected', () => {
  // Arrange
  const taken = new Set<string>()

  // Act
  const reasons = ['', 'catalog', 'chat', 'camera_audio', 'camera_cmaf', 'camera_timeline'].map((name) =>
    rejectedName(name, sourceTrackNames(name, []), taken)
  )

  // Assert
  expect(reasons.every((reason) => reason !== undefined)).toBe(true)
})

test('a name whose tracks another source publishes is rejected', () => {
  // Arrange: `camera` publishes the rendition `camera_360p`
  const taken = new Set(['camera', 'camera_audio', 'camera_360p'])

  // Act
  const sameName = rejectedName('camera', sourceTrackNames('camera', []), taken)
  const renditionName = rejectedName('camera_360p', sourceTrackNames('camera_360p', []), taken)
  const clashingRendition = rejectedName('cam', sourceTrackNames('cam', ['era']), new Set(['cam_era']))

  // Assert
  expect(sameName).toBeDefined()
  expect(renditionName).toBeDefined()
  expect(clashingRendition).toBeDefined()
})

test('each named source collects its renditions and audio', () => {
  // Arrange
  const video = [track('camera', 1), track('camera_360p', 1), track('camera_cmaf', 2, 'cmaf'), track('screen')]
  const audio = [track('camera_audio'), track('screen_audio')]

  // Act
  const sources = catalogSources(video, audio)

  // Assert
  expect(sources).toEqual([
    { name: 'camera', label: 'camera', video: 'camera', audio: 'camera_audio' },
    { name: 'screen', label: 'screen', video: 'screen', audio: 'screen_audio' }
  ])
})

test('a source whose video is muted lasts through its audio', () => {
  // Arrange: the camera's video and renditions left the catalog
  const audio = [track('camera_audio')]

  // Act
  const sources = catalogSources([], audio)

  // Assert
  expect(sources).toEqual([{ name: 'camera', label: 'camera_audio', audio: 'camera_audio' }])
  expect(liveSource(sources[0])).toEqual({ video: 'camera', audio: 'camera_audio' })
})

test('the unnamed video and audio tracks are the source video', () => {
  // Arrange: a broadcast of `publish()` or `crates/broadcast`
  const video = [track('video', 1), track('video_low', 1)]
  const audio = [track('audio')]

  // Act
  const sources = catalogSources(video, audio)

  // Assert
  expect(sources).toEqual([{ name: 'video', label: 'video', video: 'video', audio: 'audio' }])
})

test('translated audio joins the source of the audio it translates', () => {
  // Arrange: live-translate lists each language as an alternate of the original audio
  const video = [track('video')]
  const audio = [track('audio', 1), track('en', 1), track('zh', 1)]

  // Act
  const sources = catalogSources(video, audio)

  // Assert
  expect(sources).toEqual([{ name: 'video', label: 'video', video: 'video', audio: 'audio' }])
})

test('an audio track without a video is a source of its own', () => {
  // Arrange
  const audio = [track('narration')]

  // Act
  const sources = catalogSources([], audio)

  // Assert
  expect(sources.map(({ name }) => name)).toEqual(['narration'])
  expect(liveSource(sources[0])).toEqual({ video: 'narration', audio: 'narration' })
})

test('a rendition listed before its video joins the source its audio names', () => {
  // Arrange: an unmuted camera lists its rendition before its video
  const video = [track('camera_low', 1), track('screen')]
  const audio = [track('camera_audio')]

  // Act
  const sources = catalogSources(video, audio)

  // Assert
  expect(sources.map(({ name, video }) => [name, video])).toEqual([
    ['camera', 'camera_low'],
    ['screen', 'screen']
  ])
})

test('a source whose name starts with another listed source is its own', () => {
  // Arrange
  const video = [track('screen'), track('screen_share')]
  const audio = [track('screen_audio')]

  // Act
  const sources = catalogSources(video, audio)

  // Assert
  expect(sources.map(({ name }) => name)).toEqual(['screen', 'screen_share'])
})
