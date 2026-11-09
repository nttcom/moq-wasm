import { expect, test } from '@playwright/test'
import { rejectedName, sourceTrackNames } from './sourceNames'

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
