import type { MoqVersion } from 'moq-wasm'

import { moqtVersion, writeMoqtVersion } from './moqtVersion'

/// A click stores the draft; a page already open keeps the draft it loaded with.
const tabs = document.querySelectorAll<HTMLButtonElement>('.version-switch [data-moqt-version]')
for (const tab of tabs) {
  tab.setAttribute('aria-selected', String(tab.dataset.moqtVersion === moqtVersion))
  tab.addEventListener('click', () => {
    writeMoqtVersion(tab.dataset.moqtVersion as MoqVersion)
    for (const other of tabs) {
      other.setAttribute('aria-selected', String(other === tab))
    }
  })
}
