import { expect, test } from '@playwright/test'
import { type StageInput, arrangeStage, galleryTileWidth } from './stageLayout'

const BASE: StageInput = {
  view: 'speaker',
  filmstripOpen: true,
  remoteMemberIds: ['alice', 'bob'],
  remoteSharingIds: new Set(),
  localSharing: false,
  activeSpeakerId: undefined
}

test('the gallery shows every camera and focuses none', () => {
  // Act
  const stage = arrangeStage({ ...BASE, view: 'gallery' })

  // Assert
  expect(stage.focus).toBeUndefined()
  expect(stage.others.map((tile) => tile.key)).toEqual(['local:camera', 'alice:camera', 'bob:camera'])
  expect(stage.visibleKeys.size).toBe(3)
})

test('the speaker view focuses the active speaker and lists the rest in the filmstrip', () => {
  // Act
  const stage = arrangeStage({ ...BASE, activeSpeakerId: 'bob' })

  // Assert
  expect(stage.focus?.key).toBe('bob:camera')
  expect(stage.others.map((tile) => tile.key)).toEqual(['local:camera', 'alice:camera'])
})

test('a closed filmstrip leaves only the focus visible', () => {
  // Act
  const stage = arrangeStage({ ...BASE, filmstripOpen: false, activeSpeakerId: 'bob' })

  // Assert
  expect([...stage.visibleKeys]).toEqual(['bob:camera'])
})

test('a screen share takes the focus from the gallery', () => {
  // Act
  const stage = arrangeStage({ ...BASE, view: 'gallery', remoteSharingIds: new Set(['alice']), activeSpeakerId: 'bob' })

  // Assert
  expect(stage.view).toBe('speaker')
  expect(stage.focus?.key).toBe('alice:screenshare')
  expect(stage.others.map((tile) => tile.key)).toEqual(['local:camera', 'alice:camera', 'bob:camera'])
})

test('a remote share is focused before the local one', () => {
  // Act
  const stage = arrangeStage({ ...BASE, remoteSharingIds: new Set(['bob']), localSharing: true })

  // Assert
  expect(stage.focus?.key).toBe('bob:screenshare')
  expect(stage.others[0].key).toBe('local:screenshare')
})

test('alone in the room the speaker view focuses the local camera', () => {
  // Act
  const stage = arrangeStage({ ...BASE, remoteMemberIds: [] })

  // Assert
  expect(stage.focus?.key).toBe('local:camera')
  expect(stage.others).toEqual([])
})

test('three tiles in a wide box sit in one row', () => {
  // Act / Assert: one row of three is 640 wide; two columns are limited by height to 622
  expect(galleryTileWidth(3, 1960, 720, 20)).toBe(640)
})

test('the height limits tiles in a box too short for their width', () => {
  // Act / Assert: two columns of two rows 195 px tall fit 346 px wide tiles, more than one row of four (242 px)
  expect(galleryTileWidth(4, 1000, 400, 10)).toBe(346)
})
