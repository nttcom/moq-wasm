import { type AccessMode, access, accessMode, startNewTemporaryApp, writeAccessMode } from './access'
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
    description.textContent = '誰でも視聴できる anon namespace を全員で共有します。送信したメディアは公開されます。'
    break
  case 'given':
    description.textContent = `URL のトークン（app ${access.namespaceRoot}）で Example を使います。`
    break
  case 'unavailable':
    description.textContent = access.reason
    break
  case 'temporary': {
    description.textContent =
      'あなた専用の namespace で Example を使います。共有リンクを知っている人だけが参加・視聴できます（リンクは 1 時間で無効になります）。'
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
function reloadWithoutToken(): void {
  window.location.replace(window.location.pathname + window.location.search)
}

function button(label: string, onClick: () => Promise<void>): HTMLButtonElement {
  const element = document.createElement('button')
  element.type = 'button'
  element.textContent = label
  element.addEventListener('click', () => void onClick())
  return element
}
