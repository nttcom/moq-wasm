import { expect, test } from '@playwright/test'
import { D3_FLASH_ORIGIN, answerDetectorWith, askCamera, joinCamera } from './camera-detection-e2e-arrange'
import { delayD3FlashAnswers } from './fake-d3-flash'
import { leaveAllPages } from './leave-all-pages'

const LATE_ANSWER_MS = 6_000

test.describe('MoQ Camera Detection', () => {
  test.afterEach(async ({ browser }) => {
    await leaveAllPages(browser, 'camera-detection-leave-button')
    await delayD3FlashAnswers(D3_FLASH_ORIGIN, 0)
  })

  test('follows the detector when its choice changes', async ({ page }) => {
    // Arrange
    await answerDetectorWith('はい')
    const verdict = await joinCamera(page)
    await expect(verdict).toHaveText('はい')

    // Act
    await answerDetectorWith('いいえ')

    // Assert
    await expect(verdict).toHaveText('いいえ')
  })

  test('answers with a choice of the edited prompt', async ({ page }) => {
    // Arrange
    await answerDetectorWith('緑')
    const verdict = await joinCamera(page)

    // Act
    await askCamera(page, '何色の服ですか？', '赤, 青, 緑')

    // Assert
    await expect(verdict).toHaveText('緑')
  })

  test('applies the prompt once its rejected choices are fixed', async ({ page }) => {
    // Arrange
    await answerDetectorWith('緑')
    const verdict = await joinCamera(page)
    await askCamera(page, '何色の服ですか？', '赤')

    // Act
    await askCamera(page, '何色の服ですか？', '赤, 緑')

    // Assert
    await expect(verdict).toHaveText('緑')
  })

  test('shows an answer outside the choices as undecided', async ({ page }) => {
    // Arrange
    await answerDetectorWith('たぶん')

    // Act
    const verdict = await joinCamera(page)

    // Assert
    await expect(verdict).toHaveText('判定できません')
  })

  test('shows the d3-flash status while its answers are late', async ({ page }) => {
    // Arrange
    await answerDetectorWith('はい')
    await delayD3FlashAnswers(D3_FLASH_ORIGIN, LATE_ANSWER_MS)

    // Act
    const verdict = await joinCamera(page)

    // Assert
    const d3FlashStatus = page.getByTestId('camera-detection-d3-flash-status')
    await expect(d3FlashStatus).toContainText('d3-flash')
    await delayD3FlashAnswers(D3_FLASH_ORIGIN, 0)
    await expect(verdict).toHaveText('はい', { timeout: 30_000 })
    await expect(d3FlashStatus).toBeHidden()
  })
})
