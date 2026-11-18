import './accessBar.css'
import { type Access, access, applyNamespaceRootToInputs } from './access'
import { temporaryStatus } from './temporaryStatus'

// Shows, beside the example's title, which namespace root the page uses and who can see it.
const CAUTION = '検証用の認証認可機能です。機密情報やプライバシーに関わる情報は入力しないでください'

const bar = document.createElement('aside')
bar.setAttribute('role', 'note')
render(bar, access)
applyNamespaceRootToInputs()

const hero = document.querySelector('.hero')
hero ? hero.append(bar) : document.body.prepend(bar)

function render(bar: HTMLElement, access: Access): void {
  bar.className = `access-bar ${access.kind}`
  switch (access.kind) {
    case 'public':
      bar.replaceChildren(
        label('🌐 Public'),
        text('送信したメディアは誰でも視聴できます'),
        info(
          '誰でも視聴できる anon namespace に Publish します。機密情報やプライバシーに関わる情報は入力しないでください。一覧ページで一時トークンに切り替えられます'
        )
      )
      return
    case 'given':
      bar.replaceChildren(label('🔑 トークン'), text(`URL のトークン（app ${access.namespaceRoot}）で接続します`))
      return
    case 'unavailable':
      bar.replaceChildren(label('⚠️ 一時トークン'), text(access.reason))
      return
    case 'temporary': {
      const [app, remaining, copy] = temporaryStatus(access, (minutes) => `残り ${minutes} 分`)
      bar.replaceChildren(
        label('🔒 一時トークン'),
        app,
        remaining,
        info(`URL 及びトークンを共有すると参加・視聴が可能になります。${CAUTION}`),
        copy
      )
    }
  }
}

function label(value: string): HTMLElement {
  const element = document.createElement('b')
  element.textContent = value
  return element
}

function text(value: string): HTMLElement {
  const element = document.createElement('span')
  element.textContent = value
  return element
}

/// Shows `note` while the pointer is over the mark or the mark has focus.
function info(note: string): HTMLElement {
  const element = document.createElement('span')
  element.className = 'access-info'
  element.tabIndex = 0
  element.textContent = 'ⓘ'
  element.dataset.note = note
  element.setAttribute('aria-label', note)
  return element
}
