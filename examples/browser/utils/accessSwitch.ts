import { ACCESS_MODE_PARAM, type AccessMode, access, accessMode, startNewTemporaryApp, writeAccessMode } from './access'
import { icon } from './icon'
import { temporaryStatus } from './temporaryStatus'

/// The example list's mode switch in the top bar. A change is stored and the page reloads, so `access`
/// is resolved again for the new mode, issuing a temporary app if there is none.
const CAUTION = '検証用の認証認可機能です。機密情報やプライバシーに関わる情報は入力しないでください'
const description = document.getElementById('access-description')!
const status = document.getElementById('access-status')!

for (const tab of document.querySelectorAll<HTMLButtonElement>('.access-switch [data-access-mode]')) {
  const tabMode = tab.dataset.accessMode as AccessMode
  tab.setAttribute('aria-pressed', String(tabMode === accessMode))
  tab.addEventListener('click', () => {
    if (tabMode !== accessMode) {
      writeAccessMode(tabMode)
      reloadWithoutToken()
    }
  })
}

switch (access.kind) {
  case 'public':
    describe('送信したメディアは誰でも視聴できます')
    break
  case 'given':
    describe(`URL のトークン（app ${access.namespaceRoot}）で接続します`)
    break
  case 'unavailable':
    // A red mark in place of the ⓘ, so the failure shows without taking room on the page.
    description.dataset.tip = access.reason
    description.setAttribute('aria-label', access.reason)
    description.classList.add('danger')
    description.replaceChildren(icon('circle-alert'))
    break
  case 'temporary': {
    describe('URL を共有した相手だけが参加・視聴できます')
    const shown = temporaryStatus(access, (minutes) => `リンクの有効期限 残り ${minutes} 分`)
    const renew = button('トークンを作り直す', async () => {
      await startNewTemporaryApp()
      reloadWithoutToken()
    })
    renew.title =
      '今開いている会議はそのまま続きます。これから開く Example は、今の共有リンクや録画とは別の場所になります'
    status.replaceChildren(...shown, renew)
  }
}

/// Puts what the mode means, and the caution every mode shares, in the ⓘ beside the switch.
function describe(note: string): void {
  const text = `${note}\n${CAUTION}`
  description.dataset.tip = text
  description.setAttribute('aria-label', text)
}

/// The fragment names the current app; reloading with it would adopt that app again as a shared link.
/// Also drops the access mode a link asked for, which would override the tab just chosen.
function reloadWithoutToken(): void {
  const query = new URLSearchParams(window.location.search)
  query.delete(ACCESS_MODE_PARAM)
  window.location.replace(window.location.pathname + (query.size > 0 ? `?${query}` : ''))
}

function button(label: string, onClick: () => Promise<void>): HTMLButtonElement {
  const element = document.createElement('button')
  element.type = 'button'
  element.append(icon('refresh-cw'), label)
  element.addEventListener('click', () => void onClick())
  return element
}
