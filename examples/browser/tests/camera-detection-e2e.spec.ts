import { expect, test } from '@playwright/test'
import {
  SYSTEM_ONE_ORIGIN,
  answerDetectorWith,
  askCamera,
  clickJoinCamera,
  joinCamera,
  placeBoxes
} from './camera-detection-e2e-arrange'
import { delaySystemOneAnswers } from './fake-system-one'
import { leaveAllPages } from './leave-all-pages'

const LATE_ANSWER_MS = 6_000

test.describe('MoQ Camera Detection', () => {
  test.afterEach(async ({ browser }) => {
    await leaveAllPages(browser, 'camera-detection-leave-button')
    await delaySystemOneAnswers(SYSTEM_ONE_ORIGIN, 0)
  })

  test('tells a page in a second room that no bot is free', async ({ browser, page }) => {
    // Arrange
    await joinCamera(page)
    const otherPage = await browser.newPage()

    // Act
    await clickJoinCamera(otherPage, 'e2e-other')

    // Assert
    await expect(otherPage.locator('#bot-room')).toHaveText('空いているボットがありません')
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

  test('draws the boxes of a boxes question next to the choice answer', async ({ page }) => {
    // Arrange
    await answerDetectorWith('はい')
    await placeBoxes({ 人: [[0.2, 0.2, 0.5, 0.9]] })
    await joinCamera(page)

    // Act
    await page.getByTestId('camera-detection-add-question-button').click()
    await page.getByTestId('camera-detection-question-input').nth(1).fill('人')
    await page.getByTestId('camera-detection-apply-button').click()

    // Assert
    const verdicts = page.getByTestId('camera-detection-verdict')
    await expect(verdicts).toHaveText(['はい', '人 1'])
    await expect(page.getByTestId('camera-detection-box')).toHaveText(['人'])
  })

  test('shows an answer outside the choices as undecided', async ({ page }) => {
    // Arrange
    await answerDetectorWith('たぶん')

    // Act
    const verdict = await joinCamera(page)

    // Assert
    await expect(verdict).toHaveText('判定できません')
  })

  test('shows the System One status while its answers are late', async ({ page }) => {
    // Arrange
    await answerDetectorWith('はい')
    await delaySystemOneAnswers(SYSTEM_ONE_ORIGIN, LATE_ANSWER_MS)

    // Act
    const verdict = await joinCamera(page)

    // Assert
    const systemOneStatus = page.getByTestId('camera-detection-system-one-status')
    await expect(systemOneStatus).toContainText('d1-2B')
    await delaySystemOneAnswers(SYSTEM_ONE_ORIGIN, 0)
    await expect(verdict).toHaveText('はい', { timeout: 30_000 })
    await expect(systemOneStatus).toBeHidden()
  })
})
