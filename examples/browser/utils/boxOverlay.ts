import './boxOverlay.css'

/// [left, top, right, bottom] as fractions of the picture, from its top-left corner.
export type BoxEdges = [number, number, number, number]

export type OverlayBox = { edges: BoxEdges; label: string; color: string }

// About one label's height as a share of the picture's.
const LABEL_HEIGHT_SHARE = 0.05

/// Golden-angle hues keep neighboring indexes' colors far apart.
export function boxColor(index: number): string {
  return `hsl(${(index * 137) % 360} 70% 45%)`
}

/// `overlay` must cover the picture exactly (a `.box-overlay` in a positioned parent the picture
/// fills at its own aspect ratio), so percentages of it are fractions of the picture.
export function drawBoxes(overlay: HTMLElement, boxes: OverlayBox[], testId: string): void {
  overlay.replaceChildren(
    ...boxes.map(({ edges: [left, top, right, bottom], label, color }) => {
      const box = document.createElement('div')
      box.className = 'overlay-box'
      box.dataset.testid = testId
      box.style.cssText = `left: ${left * 100}%; top: ${top * 100}%; width: ${(right - left) * 100}%; height: ${(bottom - top) * 100}%`
      box.style.setProperty('--box-color', color)
      if (top < LABEL_HEIGHT_SHARE) {
        box.dataset.label = 'below'
      }
      const name = document.createElement('span')
      name.textContent = label
      box.append(name)
      return box
    })
  )
}
