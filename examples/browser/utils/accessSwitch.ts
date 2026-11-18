import { ACCESS_MODE_PARAM, type AccessMode, access, accessMode, startNewTemporaryApp, writeAccessMode } from './access'
import { icon } from './icon'
import { temporaryStatus } from './temporaryStatus'

// The example list's mode switch in the top bar. A change is stored and the page reloads, so access is
// resolved again for the new mode.

const CAUTION = '検証用の認証認可機能です。機密情報やプライバシーに関わる情報は入力しないでください'
/// Set once a reader has closed the note, so it opens by itself only on their first visit.
const NOTE_CLOSED_KEY = 'moqt.accessNoteClosed'
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
    // Everything sent is open to everyone, so the mark warns instead of informing.
    describe('送信したメディアは誰でも視聴できます')
    tone('warning', 'triangle-alert')
    break
  case 'given':
    describe(`URL のトークン（app ${access.namespaceRoot}）で接続します`)
    break
  case 'unavailable':
    // A red mark in place of the ⓘ, so the failure shows without taking room on the page.
    description.dataset.tip = access.reason
    description.setAttribute('aria-label', access.reason)
    tone('danger', 'circle-alert')
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

// The note opens without a hover on a first visit, and on every visit while the token has failed.
if (access.kind === 'unavailable' || localStorage.getItem(NOTE_CLOSED_KEY) === null) {
  openNote()
}

/// Shows the ⓘ's text as an open note under the top bar's end, with a button that closes it.
function openNote(): void {
  const note = document.createElement('div')
  note.className = ['tip-note', ...['warning', 'danger'].filter((name) => description.classList.contains(name))].join(
    ' '
  )
  note.setAttribute('role', 'note')
  const text = document.createElement('span')
  text.textContent = description.dataset.tip ?? ''
  const close = document.createElement('button')
  close.type = 'button'
  close.setAttribute('aria-label', '閉じる')
  close.title = '閉じる'
  close.append(icon('x'))
  close.addEventListener('click', () => {
    note.remove()
    description.classList.remove('open')
    if (access.kind !== 'unavailable') {
      localStorage.setItem(NOTE_CLOSED_KEY, '1')
    }
    // Focus goes to the chosen mode, not the ⓘ, whose focus would show the same text again.
    document.querySelector<HTMLButtonElement>('.access-switch [aria-pressed="true"]')?.focus()
  })
  note.append(text, close)
  description.classList.add('open')
  description.parentElement!.append(note)
}

function tone(name: 'warning' | 'danger', iconName: string): void {
  description.classList.add(name)
  description.replaceChildren(icon(iconName))
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
