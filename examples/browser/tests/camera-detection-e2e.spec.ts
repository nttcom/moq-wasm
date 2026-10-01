import { expect, test } from '@playwright/test'
import { answerDetectorWith, joinCamera } from './camera-detection-e2e-arrange'

test.describe('MoQ Camera Detection', () => {
  test('follows the detector when the person leaves', async ({ page }) => {
    // Arrange
    await answerDetectorWith('person')
    const verdict = await joinCamera(page)
    await expect(verdict).toHaveText('人が映っています')

    // Act
    await answerDetectorWith('no_person')

    // Assert
    await expect(verdict).toHaveText('人は映っていません')
  })

  test('shows an answer outside the choices as undecided', async ({ page }) => {
    // Arrange
    await answerDetectorWith('maybe')

    // Act
    const verdict = await joinCamera(page)

    // Assert
    await expect(verdict).toHaveText('判定できません')
  })
})
