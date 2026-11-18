import { applyNamespaceRootToInputs } from './access'
import { createAccessBar } from './accessBar'

/// The bar at the top of every example: the way back to the list, where the page sits, and the access chip.
///
/// The page names itself with `<body data-title>` (else its `<title>`), names the page above it with
/// `data-parent-title` and `data-parent-href`, and asks for a full-width bar with `data-shell="wide"`.
/// It links utils/theme.css itself, so the colors apply before this module runs.
const LOGO =
  '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.4" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><path d="M4 17 9 7l3 6 3-6 5 10"/></svg>'

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
brand.innerHTML = `<span class="app-logo">${LOGO}</span><span class="app-brand-name">MoQT Playground</span>`

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
end.append(createAccessBar())

inner.append(brand, breadcrumb, end)
shell.append(inner)
document.body.prepend(shell)
applyNamespaceRootToInputs()

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

/// Carries the page's query (a relay URL, a token mint URL) to the pages the bar links to, as
/// utils/preserveQueryLinks.js does for the links in the page.
function withPageQuery(href: string): string {
  const url = new URL(href, window.location.href)
  for (const [name, value] of new URLSearchParams(window.location.search)) {
    url.searchParams.set(name, value)
  }
  return url.toString()
}
