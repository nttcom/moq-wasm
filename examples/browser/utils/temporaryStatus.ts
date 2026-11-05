import { type Access, shareUrl } from './access'

type TemporaryAccess = Extract<Access, { kind: 'temporary' }>

/// The room, the link's remaining minutes (as `remainingText` words them) and a button that copies
/// the link, shown by the example list and by every example's access bar.
export function temporaryStatus(access: TemporaryAccess, remainingText: (minutes: number) => string): HTMLElement[] {
  const room = document.createElement('span')
  room.append('ルーム ')
  const code = document.createElement('code')
  code.textContent = `${access.namespaceRoot.slice(0, 11)}…`
  code.title = access.namespaceRoot
  room.append(code)
  const remaining = document.createElement('span')
  const update = () => {
    const minutes = Math.max(Math.ceil((access.current().expiresAt * 1000 - Date.now()) / 60_000), 0)
    remaining.textContent = remainingText(minutes)
  }
  update()
  window.setInterval(update, 30_000)
  const copy = document.createElement('button')
  copy.type = 'button'
  copy.textContent = '共有リンクをコピー'
  copy.addEventListener('click', async () => {
    await navigator.clipboard.writeText(shareUrl(access.current()))
    copy.textContent = 'コピーしました'
    window.setTimeout(() => (copy.textContent = '共有リンクをコピー'), 2000)
  })
  return [room, remaining, copy]
}
