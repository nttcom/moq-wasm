import { expect, test } from '@playwright/test'
import { answerDetectorWith, askCamera, joinCamera } from './camera-detection-e2e-arrange'

test.describe('MoQ Camera Detection', () => {
  test('follows the detector when its choice changes', async ({ page }) => {
    // Arrange
    await answerDetectorWith('1')
    const verdict = await joinCamera(page)
    await expect(verdict).toHaveText('はい')

    // Act
    await answerDetectorWith('2')

    // Assert
    await expect(verdict).toHaveText('いいえ')
  })

  test('answers with a choice of the edited prompt', async ({ page }) => {
    // Arrange
    await answerDetectorWith('3')
    const verdict = await joinCamera(page)

    // Act
    await askCamera(page, '何色の服ですか？', '赤, 青, 緑')

    // Assert
    await expect(verdict).toHaveText('緑')
  })

  test('applies the prompt once its rejected choices are fixed', async ({ page }) => {
    // Arrange
    await answerDetectorWith('2')
    const verdict = await joinCamera(page)
    await askCamera(page, '何色の服ですか？', '赤')

    // Act
    await askCamera(page, '何色の服ですか？', '赤, 緑')

    // Assert
    await expect(verdict).toHaveText('緑')
  })

  test('shows a number outside the choices as undecided', async ({ page }) => {
    // Arrange
    await answerDetectorWith('3')

    // Act
    const verdict = await joinCamera(page)

    // Assert
    await expect(verdict).toHaveText('判定できません')
  })
})
