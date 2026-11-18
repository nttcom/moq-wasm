import { type Access, CAUTION, access } from './access'
import { icon, tip } from './icon'
import { temporaryStatus } from './temporaryStatus'

/// The chip at the end of the app shell that shows which namespace root the page uses and who can see it.
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
          '誰でも視聴できる anon namespace に Publish します。機密情報やプライバシーに関わる情報は入力しないでください。一覧ページで一時トークンに切り替えられます',
          'warning'
        )
      ]
    case 'given':
      return [dot(), label('トークン'), detail(`URL のトークン（app ${access.namespaceRoot}）で接続します`)]
    case 'unavailable':
      return [dot(), label('一時トークン'), detail(access.reason), info(access.reason, 'danger')]
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

/// The ⓘ of the chip, opening leftward from its end. A `warning` (the public app, open to everyone) or a
/// `danger` (no token) mark takes that color and icon in place of the ⓘ.
function info(note: string, tone?: 'warning' | 'danger'): HTMLElement {
  const element = tip(note)
  element.classList.add('end')
  if (tone) {
    element.classList.add(tone)
    element.replaceChildren(icon(tone === 'warning' ? 'triangle-alert' : 'circle-alert'))
  }
  return element
}
