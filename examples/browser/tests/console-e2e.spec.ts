import { expect, test } from '@playwright/test'
import {
  appIdOf,
  appPages,
  createApp,
  deleteApp,
  issueToken,
  openAppPage,
  openAppSwitcher,
  rotateSecret,
  selectApp,
  signWithSecret,
  verifyWithVts
} from './console-e2e-arrange'

test('tokens of an app pass the VTS until its secret is rotated or the app is deleted', async ({ page }) => {
  // Arrange
  await page.goto('/')
  const secret = await createApp(page, 'e2e')

  // Act
  const issued = await issueToken(page, 'site1')
  const appId = appIdOf(issued)
  const selfSigned = signWithSecret(appId, secret, 'site1')

  // Assert: both the console's token and one signed with the secret shown once are accepted
  expect(await verifyWithVts(issued)).toEqual({ status: 200, appId, publish: 'site1', error: undefined })
  expect((await verifyWithVts(selfSigned)).status).toBe(200)

  // Act
  const rotated = await rotateSecret(page)

  // Assert: tokens signed with the old secret stop working, the new secret's do not
  expect(rotated).not.toBe(secret)
  expect(await verifyWithVts(issued)).toMatchObject({ status: 401, error: 'invalid_signature' })
  expect(await verifyWithVts(selfSigned)).toMatchObject({ status: 401, error: 'invalid_signature' })
  const signedAfterRotation = signWithSecret(appId, rotated, 'site1')
  expect((await verifyWithVts(signedAfterRotation)).status).toBe(200)

  // Act
  await deleteApp(page)

  // Assert
  await expect(page.getByRole('heading', { name: '最初の app を作成' })).toBeVisible()
  expect(await verifyWithVts(signedAfterRotation)).toMatchObject({ status: 401, error: 'unknown_app' })
})

test('apps of one employee keep their own appId, secret and tokens', async ({ page }) => {
  // Arrange
  await page.goto('/')
  const firstSecret = await createApp(page, 'e2e-first')
  const firstToken = await issueToken(page, 'site1')
  const firstId = appIdOf(firstToken)
  const secondSecret = await createApp(page, 'e2e-second')
  const secondToken = await issueToken(page, 'site2')
  const secondId = appIdOf(secondToken)

  // Act
  const menu = await openAppSwitcher(page)

  // Assert
  await expect(menu.getByRole('menuitem')).toHaveText([
    new RegExp(firstId),
    new RegExp(secondId),
    'すべての app',
    'app を作成'
  ])
  await page.keyboard.press('Escape')

  for (const [shown, other] of [
    [firstId, secondId],
    [secondId, firstId]
  ]) {
    // Act
    await selectApp(page, shown)

    // Assert
    for (const label of appPages) {
      await openAppPage(page, label)
      await expect(page.locator('main')).toContainText(shown)
      await expect(page.locator('main')).not.toContainText(other)
    }
  }

  // Act / Assert
  expect(await verifyWithVts(firstToken)).toMatchObject({ status: 200, appId: firstId })
  expect(await verifyWithVts(secondToken)).toMatchObject({ status: 200, appId: secondId })
  expect(await verifyWithVts(signWithSecret(secondId, firstSecret, 'site1'))).toMatchObject({
    status: 401,
    error: 'invalid_signature'
  })
  expect(await verifyWithVts(signWithSecret(firstId, secondSecret, 'site2'))).toMatchObject({
    status: 401,
    error: 'invalid_signature'
  })

  // Act: rotate the second app's secret
  const rotated = await rotateSecret(page)

  // Assert
  expect(await verifyWithVts(secondToken)).toMatchObject({ status: 401, error: 'invalid_signature' })
  expect((await verifyWithVts(firstToken)).status).toBe(200)
  const signedAfterRotation = signWithSecret(secondId, rotated, 'site2')
  expect((await verifyWithVts(signedAfterRotation)).status).toBe(200)

  // Act
  await selectApp(page, firstId)
  await deleteApp(page)

  // Assert
  await expect((await openAppSwitcher(page)).getByRole('menuitem')).toHaveText([
    new RegExp(secondId),
    'すべての app',
    'app を作成'
  ])
  expect(await verifyWithVts(firstToken)).toMatchObject({ status: 401, error: 'unknown_app' })
  expect((await verifyWithVts(signedAfterRotation)).status).toBe(200)
})
