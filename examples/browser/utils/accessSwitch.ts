import { ACCESS_MODE_PARAM, type AccessMode, access, accessMode, startNewTemporaryApp, writeAccessMode } from './access'
import { temporaryStatus } from './temporaryStatus'

/// The example list's mode tabs. A change is stored and the page reloads, so `access` is resolved
/// again for the new mode, issuing a temporary app if there is none.
const description = document.getElementById('access-description')!
const status = document.getElementById('access-status')!

for (const tab of document.querySelectorAll<HTMLButtonElement>('.access-switch [data-access-mode]')) {
  const tabMode = tab.dataset.accessMode as AccessMode
  tab.setAttribute('aria-selected', String(tabMode === accessMode))
  tab.addEventListener('click', () => {
    if (tabMode !== accessMode) {
      writeAccessMode(tabMode)
      reloadWithoutToken()
    }
  })
}

switch (access.kind) {
  case 'public':
    description.textContent =
      '誰でも視聴できる anon namespace を全員で共有します。送信したメディアは公開されます。機密情報やプライバシーに関わる情報は入力しないでください。'
    break
  case 'given':
    description.textContent = `URL のトークン（app ${access.namespaceRoot}）で Example を使います。`
    break
  case 'unavailable':
    description.textContent = access.reason
    break
  case 'temporary': {
    description.textContent =
      '検証用の一時トークンを使用します。URL 及びトークンを共有すると参加・視聴が可能になります。検証用の認証認可機能であるため、機密情報やプライバシーに関わる情報は入力しないでください。'
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
  element.textContent = label
  element.addEventListener('click', () => void onClick())
  return element
}
