/// Importing this entry registers `<moq-player>`; `moq-wasm/media` stays free of side effects.
/// A page that shows it imports `moq-wasm/media/player.css`, and with `controls` also
/// `moq-wasm/media/controls.css`.
import { MoqPlayerElement } from './moqPlayerElement'

export { MoqPlayerElement }

if (!customElements.get('moq-player')) {
  customElements.define('moq-player', MoqPlayerElement)
}

declare global {
  interface HTMLElementTagNameMap {
    'moq-player': MoqPlayerElement
  }
}
