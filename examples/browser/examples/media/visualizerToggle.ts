import { element } from '../../utils/page'

const EYE_SVG = `
  <svg viewBox="0 0 24 24" aria-hidden="true">
    <path d="M2 12s3.5-6 10-6 10 6 10 6-3.5 6-10 6-10-6-10-6z"></path>
    <circle cx="12" cy="12" r="3"></circle>
  </svg>
`

const EYE_OFF_SVG = `
  <svg viewBox="0 0 24 24" aria-hidden="true">
    <path d="M3 3l18 18"></path>
    <path d="M10.6 10.6A2 2 0 0 0 12 14a2 2 0 0 0 1.4-.6"></path>
    <path d="M9.9 5.3A11.6 11.6 0 0 1 12 5c6.5 0 10 7 10 7a15 15 0 0 1-4 4.9"></path>
    <path d="M6.6 6.6A15.1 15.1 0 0 0 2 12s3.5 7 10 7a11.3 11.3 0 0 0 5.1-1.2"></path>
  </svg>
`

export function setUpVisualizerToggle(
  buttonId: string,
  panelId: string,
  subject: string,
  onShow: () => void
): () => boolean {
  const button = element<HTMLButtonElement>(buttonId)
  const panel = element(panelId)
  let visible = false
  const sync = () => {
    button.innerHTML = visible ? EYE_OFF_SVG : EYE_SVG
    button.classList.toggle('active', visible)
    button.setAttribute('aria-pressed', String(visible))
    const label = `${subject}を${visible ? '非表示' : '表示'}`
    button.setAttribute('aria-label', label)
    button.title = label
  }
  sync()
  button.addEventListener('click', () => {
    visible = !visible
    panel.classList.toggle('is-visible', visible)
    sync()
    if (visible) {
      onShow()
    }
  })
  return () => visible
}
