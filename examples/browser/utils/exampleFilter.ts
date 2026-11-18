import { access } from './access'

/// The example list's category buttons and search box: a card shows when it is in the chosen category
/// and its text contains every word typed, and a section with no card left hides.
const buttons = [...document.querySelectorAll<HTMLButtonElement>('[data-filter]')]
const search = document.getElementById('example-search') as HTMLInputElement
const sections = [...document.querySelectorAll<HTMLElement>('[data-section]')]
const empty = document.getElementById('example-empty')!
let category = 'all'

for (const button of buttons) {
  button.addEventListener('click', () => {
    category = button.dataset.filter!
    for (const other of buttons) {
      other.setAttribute('aria-pressed', String(other === button))
    }
    apply()
  })
}
search.addEventListener('input', apply)
document.getElementById('example-reset')!.addEventListener('click', () => {
  search.value = ''
  buttons[0].click()
})

// "/" focuses the search box, as on most documentation sites; Escape clears it.
document.addEventListener('keydown', (event) => {
  if (event.key === '/' && !(event.target instanceof HTMLInputElement)) {
    event.preventDefault()
    search.focus()
  }
})
search.addEventListener('keydown', (event) => {
  if (event.key === 'Escape') {
    search.value = ''
    apply()
  }
})

function apply(): void {
  const words = search.value.toLowerCase().split(/\s+/).filter(Boolean)
  let shown = 0
  for (const section of sections) {
    let sectionShown = 0
    for (const card of section.querySelectorAll<HTMLElement>('.example')) {
      const text = card.textContent!.toLowerCase()
      const visible =
        (category === 'all' || card.dataset.category === category) && words.every((word) => text.includes(word))
      card.hidden = !visible
      if (visible) sectionShown += 1
    }
    section.hidden = sectionShown === 0
    shown += sectionShown
  }
  empty.hidden = shown > 0
}

// The recorder records only rooms with a token, so the public app has no recording to play.
if (access.kind === 'public') {
  for (const card of document.querySelectorAll<HTMLAnchorElement>('.requires-token')) {
    card.setAttribute('aria-disabled', 'true')
    card.tabIndex = -1
    card.addEventListener('click', (event) => event.preventDefault())
  }
}
