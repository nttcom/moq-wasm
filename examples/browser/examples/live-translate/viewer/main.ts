import { MoqSession } from 'moq-wasm'
import 'moq-wasm/media/player.css'
import { element, getErrorMessage, setStatusText } from '../../../utils/page'
import { startPageTelemetry } from '../../../utils/pageTelemetry'
import { setUpRelayUrlControls } from '../../../utils/relayPresets'
import { BROADCAST_ID_PARAM } from '../liveTranslate'
import { TranslationPlayer, type TranslationPlayerStats } from './translationPlayer'

declare global {
  interface Window {
    liveTranslateViewer?: { stats(): TranslationPlayerStats; selectLanguage(lang: string): Promise<void> }
  }
}

const MONITOR_ORIGINAL_PARAM = 'monitorOriginal'
const STATS_INTERVAL_MS = 500

const urlInput = element<HTMLInputElement>('url')
const languageSelect = element<HTMLSelectElement>('language')
const monitorOriginal = element<HTMLInputElement>('monitor-original')
let session: MoqSession | undefined
const player = new TranslationPlayer(element<HTMLDivElement>('stage'), {
  onStateChange: () => renderLanguages(),
  onLog: (level, message) => appendLog(level, message)
})
window.liveTranslateViewer = { stats: () => player.stats(), selectLanguage: (lang) => player.selectLanguage(lang) }

startPageTelemetry('live-translate-viewer', () => urlInput.value)
setUpRelayUrlControls(urlInput, element('urlPresets'))
element<HTMLInputElement>('broadcast-id').value = new URLSearchParams(location.search).get(BROADCAST_ID_PARAM) ?? ''
element<HTMLButtonElement>('watchBtn').addEventListener('click', () => void watch())
element<HTMLButtonElement>('stopBtn').addEventListener('click', () => void stop())
languageSelect.addEventListener('change', () => void player.selectLanguage(languageSelect.value))
monitorOriginal.checked = new URLSearchParams(location.search).get(MONITOR_ORIGINAL_PARAM) === '1'
player.setMonitorOriginal(monitorOriginal.checked)
monitorOriginal.addEventListener('change', () => player.setMonitorOriginal(monitorOriginal.checked))
setInterval(renderStats, STATS_INTERVAL_MS)

async function watch(): Promise<void> {
  const broadcastId = element<HTMLInputElement>('broadcast-id').value.trim()
  const url = urlInput.value.trim()
  if (!broadcastId) {
    setStatusText('viewer-status', 'Enter a broadcast ID')
    return
  }
  try {
    await stop()
    session = await MoqSession.connect(url)
    setStatusText('viewer-status', `Watching ${broadcastId}`)
    await player.start(session, broadcastId)
  } catch (error) {
    setStatusText('viewer-status', `Failed: ${getErrorMessage(error)}`)
  }
}

async function stop(): Promise<void> {
  await player.stop()
  session?.close()
  session = undefined
}

function renderLanguages(): void {
  const options = player.languages
  if (languageSelect.options.length !== options.length) {
    languageSelect.replaceChildren(
      ...options.map((option) => new Option(option.label, option.lang, false, option.lang === player.language))
    )
  }
  languageSelect.value = player.pendingLanguage ?? player.language
  setStatusText(
    'language-status',
    player.pendingLanguage ? `Switching to ${player.pendingLanguage}…` : `Listening: ${player.language}`
  )
}

function renderStats(): void {
  const stats = player.stats()
  if (stats.playingAudioMicros === undefined) {
    setStatusText('viewer-stats', '')
    return
  }
  const ms = (value: number | undefined) =>
    value === undefined ? '--' : `${value >= 0 ? '+' : '-'}${Math.abs(Math.round(value))} ms`
  const size = stats.frameSize ? `${stats.frameSize.width}x${stats.frameSize.height}` : 'no video yet'
  setStatusText(
    'viewer-stats',
    [
      `遅延 ${stats.viewerDelayMs === undefined ? '--' : (stats.viewerDelayMs / 1_000).toFixed(2)} s`,
      `A/V ${ms(stats.syncOffsetMs)}`,
      `訳と原音声のずれ ${ms(stats.translationOffsetMs)}`,
      `聞いている言語 ${stats.language}${stats.pendingLanguage ? `（${stats.pendingLanguage} へ切替中）` : ''}`,
      `直近の切替 ${stats.lastSwitchSeconds === undefined ? '--' : `${stats.lastSwitchSeconds.toFixed(1)} s`}`,
      `遅れて捨てた 音声 ${stats.droppedLateAudio} / 映像 ${stats.droppedLateVideo}`,
      `映像 ${size} ${Math.round(stats.receivedVideoKbps)} kbps`
    ].join(' · ')
  )
}

function appendLog(level: 'info' | 'warn' | 'error', message: string): void {
  const entry = document.createElement('div')
  entry.className = `log-entry log-${level}`
  entry.textContent = `${new Date().toLocaleTimeString()} ${message}`
  element<HTMLDivElement>('logPanel').prepend(entry)
}
