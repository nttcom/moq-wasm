/// The bar at the top of every example: the way back to the list, where the page sits, and the access chip.
///
/// The page names itself with `<body data-title>` (else its `<title>`), names the page above it with
/// `data-parent-title` and `data-parent-href`, and asks for a full-width bar with `data-shell="wide"`.
/// It links utils/theme.css itself, so the colors apply before this module runs.
///
/// The bar is drawn at once; the access chip follows once utils/access.ts has resolved the page's
/// token, which may wait for the token mint.

import { withPageQuery } from './pageQuery'

const shell = document.createElement('header')
shell.className = 'app-shell'
const inner = document.createElement('div')
inner.className = 'app-shell-inner'
if (document.body.dataset.shell === 'wide') {
  inner.classList.add('wide')
}

const home = withPageQuery(import.meta.env.BASE_URL)
const brand = document.createElement('a')
brand.className = 'app-brand'
brand.href = home
brand.innerHTML = `<span class="app-logo"><svg aria-hidden="true"><use href="${import.meta.env.BASE_URL}icons.svg#icon-logo" /></svg></span><span class="app-brand-name">MoQT Playground</span>`

const breadcrumb = document.createElement('nav')
breadcrumb.className = 'app-breadcrumb'
breadcrumb.setAttribute('aria-label', 'パンくずリスト')
const trail = document.createElement('ol')
trail.append(crumb('Examples', home))
const { parentTitle, parentHref } = document.body.dataset
if (parentTitle && parentHref) {
  trail.append(crumb(parentTitle, withPageQuery(parentHref)))
}
trail.append(crumb(document.body.dataset.title ?? document.title))
breadcrumb.append(trail)

const end = document.createElement('div')
end.className = 'app-shell-end'

inner.append(brand, breadcrumb, end)
shell.append(inner)
document.body.prepend(shell)

void Promise.all([import('./access'), import('./accessBar')]).then(
  ([{ applyNamespaceRootToInputs }, { createAccessBar }]) => {
    end.append(createAccessBar())
    applyNamespaceRootToInputs()
  }
)

function crumb(text: string, href?: string): HTMLLIElement {
  const item = document.createElement('li')
  if (href) {
    const link = document.createElement('a')
    link.href = href
    link.textContent = text
    item.append(link)
  } else {
    const current = document.createElement('span')
    current.setAttribute('aria-current', 'page')
    current.textContent = text
    item.append(current)
  }
  return item
}
