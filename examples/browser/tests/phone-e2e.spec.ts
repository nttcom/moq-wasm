import { execFileSync } from 'node:child_process'
import { type Page, expect, test } from '@playwright/test'
import { PHONE_INDEX_PATH } from '../playwright.helpers'

// Run by tests/sip-gateway-e2e/run.sh, against the local Asterisk trunk
// (tests/sip-gateway-e2e/asterisk) and the dev app server (app-server.mjs).
const moqtUrl = process.env.PHONE_E2E_MOQT_URL ?? 'https://127.0.0.1:4433'
const appServerUrl = process.env.PHONE_E2E_APP_SERVER_URL ?? 'http://127.0.0.1:8097'
const composeFile = process.env.PHONE_E2E_COMPOSE_FILE ?? ''
/// Answers and sends back what it hears (extensions.conf).
const ECHO = '+819000000001'
const BUSY = '+819000000003'
/// Hangs up once it hears the digit 5.
const HEARS_DTMF = '+819000000005'
/// The app's number (apps.json) and the phone that calls it (pjsip.conf).
const APP_NUMBER = '+81300000001'
const CALLER = '+819011112222'

async function joinPage(page: Page): Promise<void> {
  const room = `e2e-${Date.now().toString(36)}`
  await page.goto(`${PHONE_INDEX_PATH}?${new URLSearchParams({ moqtUrl, appServerUrl, room })}`)
  await expect(page.locator('#url')).toHaveValue(moqtUrl)
  await page.getByTestId('phone-join-button').click()
  await expect(page.getByTestId('phone-status')).toHaveText('接続中')
}

async function expectState(page: Page, state: string, reason = ''): Promise<void> {
  const shown = page.getByTestId('phone-state')
  await expect(shown).toHaveAttribute('data-state', state)
  await expect(shown).toHaveAttribute('data-reason', reason)
}

async function expectPhoneAudio(page: Page): Promise<void> {
  await expect
    .poll(async () => Number(await page.getByTestId('phone-audio').getAttribute('data-kbps')))
    .toBeGreaterThan(0)
}

function asterisk(command: string): string {
  return execFileSync('docker', ['compose', '-f', composeFile, 'exec', '-T', 'trunk', 'asterisk', '-rx', command], {
    encoding: 'utf8'
  })
}

test('the page calls a phone, hears it and hangs up', async ({ page }) => {
  // Arrange
  await joinPage(page)

  // Act
  await page.getByTestId('phone-number').fill(ECHO)
  await page.getByTestId('phone-call-button').click()
  await expectState(page, 'answered')
  await expectPhoneAudio(page)
  await page.getByTestId('phone-hangup-button').click()

  // Assert
  await expectState(page, 'ended', 'hangup')
  await expect.poll(() => asterisk('core show channels count')).toContain('0 active calls')
})

test('the phone hangs up on the digit the page sends', async ({ page }) => {
  // Arrange
  await joinPage(page)
  await page.getByTestId('phone-number').fill(HEARS_DTMF)
  await page.getByTestId('phone-call-button').click()
  await expectState(page, 'answered')

  // Act
  await page.getByTestId('phone-dtmf-5').click()

  // Assert
  await expectState(page, 'ended', 'hangup')
})

test('a busy phone ends the call with 486', async ({ page }) => {
  // Arrange
  await joinPage(page)

  // Act
  await page.getByTestId('phone-number').fill(BUSY)
  await page.getByTestId('phone-call-button').click()

  // Assert
  await expectState(page, 'ended', '486')
})

test('the page answers a call to the app’s number', async ({ page }) => {
  // Arrange
  await joinPage(page)

  // Act
  asterisk(`channel originate PJSIP/${APP_NUMBER}@gateway application Echo`)
  await expect(page.getByTestId('phone-incoming')).toContainText(CALLER)
  await page.getByTestId('phone-answer-button').click()

  // Assert
  await expectState(page, 'answered')
  await expectPhoneAudio(page)
  await page.getByTestId('phone-hangup-button').click()
  await expectState(page, 'ended', 'hangup')
  await expect.poll(() => asterisk('core show channels count')).toContain('0 active calls')
})
