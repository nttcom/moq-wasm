/// The example list's category buttons: each shows only its section, and "all" shows every section.
const buttons = [...document.querySelectorAll<HTMLButtonElement>('[data-filter]')]
const sections = [...document.querySelectorAll<HTMLElement>('[data-section]')]

for (const button of buttons) {
  const filter = button.dataset.filter!
  button.addEventListener('click', () => {
    for (const other of buttons) {
      other.setAttribute('aria-pressed', String(other === button))
    }
    for (const section of sections) {
      section.hidden = filter !== 'all' && section.dataset.section !== filter
    }
  })
  // The counts follow the cards, so adding an example updates them.
  const shown = sections.filter((section) => filter === 'all' || section.dataset.section === filter)
  button.querySelector('.count')!.textContent = String(
    shown.reduce((count, section) => count + section.querySelectorAll('.example').length, 0)
  )
}

// The recorder records only rooms with a token, so the public app has no recording to play. The mode
// is read from <html>, set before the first paint, so the list works while a token is still being issued.
if (document.documentElement.dataset.accessMode === 'public') {
  for (const card of document.querySelectorAll<HTMLAnchorElement>('.requires-token')) {
    card.removeAttribute('href')
    card.setAttribute('aria-disabled', 'true')
  }
}
