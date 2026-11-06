import { expect, test } from '@playwright/test'
import type { MediaCatalogTrack } from '../catalog'
import { SeekTimeline } from './seekTimeline'

function timeline(name: string, depends: string[]): MediaCatalogTrack {
  return { name, label: name, packaging: 'mediatimeline', depends }
}

test('a later catalog updates the tracks the followed timeline depends on', () => {
  // Arrange: the source's video is muted when the first catalog arrives
  const seek = new SeekTimeline()
  const first = seek.followMediaTimeline(timeline('video_timeline', ['audio']))

  // Act
  const again = seek.followMediaTimeline(timeline('video_timeline', ['video', 'audio']))
  const other = seek.followMediaTimeline(timeline('screenshare_timeline', ['screenshare']))

  // Assert
  expect([first, again, other]).toEqual([true, false, false])
  expect(seek.mediaTimelineTrackName).toBe('video_timeline')
  expect(seek.stampedByMediaTimeline('video')).toBe(true)
  expect(seek.stampedByMediaTimeline('screenshare')).toBe(false)
})
