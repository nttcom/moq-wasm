import { withPageQuery } from './pageQuery'

if (Array.from(new URLSearchParams(window.location.search)).length > 0) {
  document.querySelectorAll('a[href]').forEach((link) => {
    const href = link.getAttribute('href')
    if (!href || href.startsWith('#')) {
      return
    }
    if (new URL(href, window.location.href).origin !== window.location.origin) {
      return
    }
    link.href = withPageQuery(href)
  })
}
