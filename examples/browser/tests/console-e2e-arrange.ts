import { createHmac } from 'node:crypto'
import { expect, type Locator, type Page } from '@playwright/test'

const verifyUrl = process.env.CONSOLE_E2E_VERIFY_URL ?? 'http://127.0.0.1:8081/verify'

export type Verification = { status: number; appId?: string; publish?: unknown; error?: string }

export async function verifyWithVts(token: string): Promise<Verification> {
  const response = await fetch(verifyUrl, {
    method: 'POST',
    headers: { 'content-type': 'application/json' },
    body: JSON.stringify({ token })
  })
  const body = (await response.json()) as { appId?: string; claims?: { publish?: unknown }; error?: string }
  return { status: response.status, appId: body.appId, publish: body.claims?.publish, error: body.error }
}

const base64url = (data: string) => Buffer.from(data).toString('base64url')

export function signWithSecret(appId: string, secret: string, publish: string): string {
  const now = Math.floor(Date.now() / 1000)
  const signingInput = `${base64url('{"alg":"HS256"}')}.${base64url(JSON.stringify({ appId, publish, iat: now, exp: now + 3600 }))}`
  const signature = createHmac('sha256', secret).update(signingInput).digest('base64url')
  return `${signingInput}.${signature}`
}

export function appIdOf(token: string): string {
  return JSON.parse(Buffer.from(token.split('.')[1], 'base64url').toString()).appId
}

async function keepShownSecret(page: Page): Promise<string> {
  const dialog = page.getByRole('dialog')
  const secret = (await dialog.locator('code').textContent()) ?? ''
  await dialog.getByLabel('secret を安全な場所に保存しました').check()
  await dialog.getByRole('button', { name: '閉じる' }).click()
  await expect(dialog).toBeHidden()
  return secret
}

const sidebar = (page: Page) => page.locator('aside')

export const appPages = ['概要', '認証（secret）', 'トークン発行'] as const

type AppPage = (typeof appPages)[number]

export async function openAppPage(page: Page, label: AppPage): Promise<void> {
  await sidebar(page).getByRole('navigation', { name: 'メニュー' }).getByRole('button', { name: label }).click()
}

export async function openAppSwitcher(page: Page): Promise<Locator> {
  await sidebar(page).locator('button[aria-haspopup="menu"]').click()
  return page.getByRole('menu')
}

export async function selectApp(page: Page, appId: string): Promise<void> {
  const menu = await openAppSwitcher(page)
  await menu.getByRole('menuitem', { name: appId }).click()
}

export async function createApp(page: Page, name: string): Promise<string> {
  const menu = await openAppSwitcher(page)
  await menu.getByRole('menuitem', { name: 'app を作成' }).click()
  await page.getByLabel('app 名').fill(name)
  await page.getByRole('button', { name: '作成して secret を表示' }).click()
  return keepShownSecret(page)
}

export async function issueToken(page: Page, path: string): Promise<string> {
  await openAppPage(page, 'トークン発行')
  await page.getByPlaceholder('live/room-1').fill(path)
  await page.getByRole('button', { name: 'トークンを発行' }).click()
  const dialog = page.getByRole('dialog', { name: 'トークンを発行しました' })
  const token = (await dialog.locator('code').textContent()) ?? ''
  await dialog.getByRole('button', { name: '閉じる' }).click()
  return token
}

export async function rotateSecret(page: Page): Promise<string> {
  await openAppPage(page, '認証（secret）')
  await page.getByRole('button', { name: '作り直す' }).click()
  await page.getByRole('dialog').getByRole('button', { name: '作り直す' }).click()
  await expect(page.getByRole('dialog', { name: '新しい secret' })).toBeVisible()
  return keepShownSecret(page)
}

export async function deleteApp(page: Page): Promise<void> {
  await openAppPage(page, '概要')
  await page.getByRole('button', { name: 'app を削除' }).click()
  await page.getByRole('dialog').getByRole('button', { name: '削除する' }).click()
}
