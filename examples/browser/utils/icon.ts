/// An icon of public/icons.svg, styled by the `.icon` rule of utils/theme.css. `name` is the symbol's id
/// without its `icon-` prefix, such as `play`.
export function icon(name: string): SVGSVGElement {
  const svg = document.createElementNS('http://www.w3.org/2000/svg', 'svg')
  svg.classList.add('icon')
  svg.setAttribute('aria-hidden', 'true')
  const use = document.createElementNS('http://www.w3.org/2000/svg', 'use')
  use.setAttribute('href', `${import.meta.env.BASE_URL}icons.svg#icon-${name}`)
  svg.append(use)
  return svg
}

/// The ⓘ of the `.tip` rule: `note` shows while the pointer is over it or it has focus.
export function tip(note: string): HTMLSpanElement {
  const element = document.createElement('span')
  element.className = 'tip'
  element.tabIndex = 0
  element.setAttribute('role', 'img')
  element.setAttribute('aria-label', note)
  element.dataset.tip = note
  element.append(icon('info'))
  return element
}
