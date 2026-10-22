import { expect, test } from '@playwright/test'
import {
  appIdOf,
  createApp,
  deleteApp,
  issueToken,
  rotateSecret,
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
