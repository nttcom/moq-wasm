const sourceEntries = Array.from(new URLSearchParams(window.location.search).entries())

if (sourceEntries.length > 0) {
  document.querySelectorAll('a[href]').forEach((link) => {
    const href = link.getAttribute('href')
    if (!href || href.startsWith('#')) {
      return
    }

    const url = new URL(href, window.location.href)
    if (url.origin !== window.location.origin) {
      return
    }

    for (const [name, value] of sourceEntries) {
      url.searchParams.set(name, value)
    }
    link.href = url.toString()
  })
}
