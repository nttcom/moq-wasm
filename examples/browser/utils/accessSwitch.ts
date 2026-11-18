import { ACCESS_MODE_PARAM, type AccessMode, access, accessMode, startNewTemporaryApp, writeAccessMode } from './access'
import { icon } from './icon'
import { temporaryStatus } from './temporaryStatus'

/// The example list's mode tabs. A change is stored and the page reloads, so `access` is resolved
/// again for the new mode, issuing a temporary app if there is none.
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
    description.textContent = '送信したメディアは誰でも視聴できます'
    break
  case 'given':
    description.textContent = `URL のトークン（app ${access.namespaceRoot}）で接続します`
    break
  case 'unavailable':
    description.textContent = access.reason
    break
  case 'temporary': {
    description.textContent = 'URL を共有した相手だけが参加・視聴できます'
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
  element.append(icon('refresh-cw'), label)
  element.addEventListener('click', () => void onClick())
  return element
}
