import './accessBar.css'
import { type Access, access, applyNamespaceRootToInputs } from './access'
import { temporaryStatus } from './temporaryStatus'

// Shows, at the top of an example, which namespace root the page uses and who can see it.
const bar = document.createElement('aside')
bar.setAttribute('role', 'note')
render(bar, access)
applyNamespaceRootToInputs()

const hero = document.querySelector('.hero')
hero ? hero.after(bar) : document.body.prepend(bar)

function render(bar: HTMLElement, access: Access): void {
  bar.className = `access-bar ${access.kind}`
  switch (access.kind) {
    case 'public':
      bar.replaceChildren(
        label('🌐 Public'),
        text(
          '誰でも視聴できる anon namespace に Publish します。送信したメディアは公開されます。一覧ページで一時トークンに切り替えられます'
        )
      )
      return
    case 'given':
      bar.replaceChildren(label('🔑 トークン'), text(`URL のトークン（app ${access.namespaceRoot}）で接続します`))
      return
    case 'unavailable':
      bar.replaceChildren(label('⚠️ 一時トークン'), text(access.reason))
      return
    case 'temporary':
      bar.replaceChildren(
        label('🔒 一時トークン'),
        ...temporaryStatus(
          access,
          (minutes) =>
            `このリンクを知っている人は誰でも参加・録画の閲覧ができます · リンクの有効期限 残り ${minutes} 分`
        )
      )
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
