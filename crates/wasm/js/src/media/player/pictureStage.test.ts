import { expect, test } from '@playwright/test'
import { PictureStage } from './pictureStage'
import { FakePictureContainer, FakePictureElement, installFakePictureDocument } from './testing/fakePictureDocument'

function arrangeStage() {
  const uninstall = installFakePictureDocument()
  const player = { paused: false }
  const stage = new PictureStage(new FakePictureContainer() as unknown as HTMLElement, () => !player.paused)
  const liveVideo = stage.liveVideo as unknown as FakePictureElement
  return { stage, liveVideo, player, uninstall }
}

test('a pause the player did not ask for plays the live video again', () => {
  // Arrange
  const { liveVideo, uninstall } = arrangeStage()

  try {
    // Act: the browser pauses the video the page moved to another parent
    liveVideo.dispatchEvent(new Event('pause'))

    // Assert
    expect(liveVideo.playCalls).toBe(1)
  } finally {
    uninstall()
  }
})

test('a pause the player asked for leaves the live video paused', () => {
  // Arrange
  const { liveVideo, player, uninstall } = arrangeStage()

  try {
    // Act
    player.paused = true
    liveVideo.dispatchEvent(new Event('pause'))

    // Assert
    expect(liveVideo.playCalls).toBe(0)
  } finally {
    uninstall()
  }
})

test('a pause after the pictures are removed leaves the live video paused', () => {
  // Arrange
  const { stage, liveVideo, uninstall } = arrangeStage()

  try {
    // Act
    stage.remove()
    liveVideo.dispatchEvent(new Event('pause'))

    // Assert
    expect(liveVideo.playCalls).toBe(0)
  } finally {
    uninstall()
  }
})
