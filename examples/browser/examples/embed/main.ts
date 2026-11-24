import 'moq-wasm/element'
import 'moq-wasm/media/player.css'
import 'moq-wasm/media/controls.css'
import { connectOptions } from '../../utils/access'
import { element, getErrorMessage, initializeMediaExamplePage, setStatus } from '../../utils/page'
import { startPageTelemetry } from '../../utils/pageTelemetry'

const url = element<HTMLInputElement>('url')
const namespace = element<HTMLInputElement>('namespace')
const player = element<HTMLElementTagNameMap['moq-player']>('player')

startPageTelemetry('embed', () => url.value.trim())
initializeMediaExamplePage('namespace')
player.token = connectOptions().token
showSnippet()

element<HTMLButtonElement>('playBtn').addEventListener('click', () => {
  player.setAttribute('src', url.value.trim())
  player.setAttribute('namespace', namespace.value.trim())
  setStatus('status', `再生中: ${namespace.value.trim()}`, 'ok')
  showSnippet()
})
element<HTMLButtonElement>('stopBtn').addEventListener('click', () => {
  player.removeAttribute('src')
  setStatus('status', '未接続', 'idle')
})
player.addEventListener('error', (event) => {
  setStatus('status', `失敗: ${getErrorMessage((event as ErrorEvent).error)}`, 'error')
})

function showSnippet(): void {
  element('snippet').textContent = [
    `<script type="module">import 'moq-wasm/element'</script>`,
    `<moq-player src="${url.value.trim()}" namespace="${namespace.value.trim()}" controls></moq-player>`
  ].join('\n')
}
