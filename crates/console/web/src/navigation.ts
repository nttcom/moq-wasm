import type { IconName } from './icons'

export type Page = 'overview' | 'auth' | 'tokens' | 'analytics' | 'observability' | 'media' | 'apps' | 'billing'

export type SoonPage = Extract<Page, 'analytics' | 'observability' | 'media' | 'billing'>

type NavItem = { page: Page; label: string; icon: IconName; soon?: boolean }

export const appNav: NavItem[] = [
  { page: 'overview', label: '概要', icon: 'gauge' },
  { page: 'auth', label: '認証（secret）', icon: 'key' },
  { page: 'tokens', label: 'トークン発行', icon: 'ticket' },
  { page: 'analytics', label: 'Analytics', icon: 'chart', soon: true },
  { page: 'observability', label: 'Observability', icon: 'activity', soon: true },
  { page: 'media', label: 'メディア処理', icon: 'film', soon: true }
]

export const accountNav: NavItem[] = [
  { page: 'apps', label: 'すべての app', icon: 'grid' },
  { page: 'billing', label: 'Billing', icon: 'card', soon: true }
]

export const pageTitles: Record<Page, string> = {
  overview: '概要',
  auth: '認証',
  tokens: 'トークン発行',
  analytics: 'Analytics',
  observability: 'Observability',
  media: 'メディア処理',
  apps: 'すべての app',
  billing: 'Billing'
}

export function isAppPage(page: Page): boolean {
  return appNav.some((item) => item.page === page)
}
