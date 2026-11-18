import { type Access, access } from './access'
import { temporaryStatus } from './temporaryStatus'

/// The chip at the end of the app shell that shows which namespace root the page uses and who can see it.
const CAUTION = '検証用の認証認可機能です。機密情報やプライバシーに関わる情報は入力しないでください'

export function createAccessBar(): HTMLElement {
  const bar = document.createElement('aside')
  bar.setAttribute('role', 'note')
  bar.setAttribute('aria-label', '接続モード')
  bar.className = `access-bar ${access.kind}`
  bar.replaceChildren(...content(access))
  return bar
}

function content(access: Access): HTMLElement[] {
  switch (access.kind) {
    case 'public':
      return [
        dot(),
        label('Public'),
        detail('送信したメディアは誰でも視聴できます'),
        info(
          '誰でも視聴できる anon namespace に Publish します。機密情報やプライバシーに関わる情報は入力しないでください。一覧ページで一時トークンに切り替えられます'
        )
      ]
    case 'given':
      return [dot(), label('トークン'), detail(`URL のトークン（app ${access.namespaceRoot}）で接続します`)]
    case 'unavailable':
      return [dot(), label('一時トークン'), detail(access.reason), info(access.reason)]
    case 'temporary': {
      const [app, remaining, copy] = temporaryStatus(access, (minutes) => `残り ${minutes} 分`)
      app.classList.add('access-detail')
      return [
        dot(),
        label('一時トークン'),
        app,
        remaining,
        info(`URL 及びトークンを共有すると参加・視聴が可能になります。${CAUTION}`),
        copy
      ]
    }
  }
}

function dot(): HTMLElement {
  const element = document.createElement('span')
  element.className = 'access-dot'
  element.setAttribute('aria-hidden', 'true')
  return element
}

function label(value: string): HTMLElement {
  const element = document.createElement('b')
  element.textContent = value
  return element
}

function detail(value: string): HTMLElement {
  const element = document.createElement('span')
  element.className = 'access-detail'
  element.textContent = value
  return element
}

/// Shows `note` while the pointer is over the mark or the mark has focus.
function info(note: string): HTMLElement {
  const element = document.createElement('span')
  element.className = 'access-info'
  element.tabIndex = 0
  element.dataset.note = note
  element.setAttribute('role', 'img')
  element.setAttribute('aria-label', note)
  element.innerHTML =
    '<svg width="15" height="15" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" aria-hidden="true"><circle cx="12" cy="12" r="10"/><path d="M12 16v-4M12 8h.01"/></svg>'
  return element
}
