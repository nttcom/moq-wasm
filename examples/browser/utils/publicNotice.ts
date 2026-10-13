import './publicNotice.css'

const NOTICE_LINES = [
  'Example は誰でも視聴できる anon Namespace に Publish します。送信したメディアは公開されるためご注意ください',
  '認証機能を使う場合は、ご自身で Relay サーバーをホスティングしてください'
]

const notice = document.createElement('aside')
notice.className = 'public-notice'
notice.setAttribute('role', 'note')
notice.textContent = NOTICE_LINES.join('\n')

const hero = document.querySelector('.hero')
hero ? hero.after(notice) : document.body.prepend(notice)
