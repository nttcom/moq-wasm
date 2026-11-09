import { MoqSession } from 'moq-wasm'
import 'moq-wasm/media/player.css'
import { element, getErrorMessage, setStatusText } from '../../../utils/page'
import { startPageTelemetry } from '../../../utils/pageTelemetry'
import { relayUrlFromQuery, setUpRelayUrlControls } from '../../../utils/relayPresets'
import { type BroadcastAccess, BROADCAST_ID_PARAM, openBroadcastAccess, translatedNamespace } from '../liveTranslate'
import { TranslationPlayer, type TranslationPlayerStats } from './translationPlayer'

declare global {
  interface Window {
    liveTranslateViewer?: { stats(): TranslationPlayerStats; selectLanguage(lang: string): Promise<void> }
  }
}

const MONITOR_ORIGINAL_PARAM = 'monitorOriginal'
const DELAY_SECONDS_PARAM = 'delay'
const STATS_INTERVAL_MS = 500
const STATS_LOG_INTERVAL_MS = 10_000

const urlInput = element<HTMLInputElement>('url')
const broadcastIdInput = element<HTMLInputElement>('broadcast-id')
const monitorOriginal = element<HTMLInputElement>('monitor-original')
const watchOverlay = element<HTMLDivElement>('watch-overlay')
const watchButton = element<HTMLButtonElement>('watchBtn')
const languageBar = element<HTMLElement>('language-bar')
const languageOptions = element<HTMLDivElement>('language-options')
const connection = element<HTMLDetailsElement>('connection')
let session: MoqSession | undefined
let access: BroadcastAccess | undefined
const delaySeconds = Number(new URLSearchParams(location.search).get(DELAY_SECONDS_PARAM))
const player = new TranslationPlayer(
  element<HTMLDivElement>('stage'),
  {
    onStateChange: () => {
      renderOverlay()
      renderLanguages()
    },
    onLog: (level, message) => appendLog(level, message),
    onCaption: (text) => renderCaption(text)
  },
  { playbackDelayMs: delaySeconds > 0 ? delaySeconds * 1_000 : undefined }
)
window.liveTranslateViewer = { stats: () => player.stats(), selectLanguage: (lang) => player.selectLanguage(lang) }

const telemetry = startPageTelemetry('live-translate-viewer', () => urlInput.value)
setUpRelayUrlControls(urlInput, element('urlPresets'))
broadcastIdInput.value = new URLSearchParams(location.search).get(BROADCAST_ID_PARAM) ?? ''
// The streamer's link carries the broadcast ID and the relay URL; a viewer who typed the URL by hand sees the form.
connection.open = !(broadcastIdInput.value && relayUrlFromQuery())
renderTitle()
broadcastIdInput.addEventListener('input', renderTitle)
watchButton.addEventListener('click', () => void watch())
element<HTMLButtonElement>('stopBtn').addEventListener('click', () => void stop())
monitorOriginal.checked = new URLSearchParams(location.search).get(MONITOR_ORIGINAL_PARAM) === '1'
player.setMonitorOriginal(monitorOriginal.checked)
monitorOriginal.addEventListener('change', () => player.setMonitorOriginal(monitorOriginal.checked))
setInterval(renderStats, STATS_INTERVAL_MS)
setInterval(logStats, STATS_LOG_INTERVAL_MS)

async function watch(): Promise<void> {
  const broadcastId = broadcastIdInput.value.trim()
  if (!broadcastId) {
    setStatusText('viewer-status', '配信 ID を入れてください（下の「接続」）')
    connection.open = true
    return
  }
  try {
    await stop()
    setStatusText('viewer-status', '接続しています')
    access = await openBroadcastAccess(broadcastId, 'viewer')
    if (access.relayUrl) {
      urlInput.value = access.relayUrl
    }
    session = await MoqSession.connect(urlInput.value.trim(), { token: access.token })
    await player.start(session, translatedNamespace(access.namespace))
    renderOverlay()
  } catch (error) {
    setStatusText('viewer-status', `接続できませんでした: ${getErrorMessage(error)}`)
  }
}

async function stop(): Promise<void> {
  await player.stop()
  session?.close()
  session = undefined
  access?.release()
  access = undefined
  languageBar.hidden = true
  watchOverlay.hidden = false
  watchButton.hidden = false
  element<HTMLElement>('viewer-stats').hidden = true
}

/// While the broadcast is not there yet the overlay stays, saying so, without its button.
function renderOverlay(): void {
  if (!player.watching) {
    return
  }
  const waiting = player.waiting
  watchOverlay.hidden = !waiting
  watchButton.hidden = waiting
  languageBar.hidden = waiting
  setStatusText('viewer-status', waiting ? '配信が始まるのを待っています' : '')
}

function renderCaption(text: string): void {
  const captions = element<HTMLElement>('captions')
  captions.textContent = text
  captions.hidden = text.length === 0
}

function renderTitle(): void {
  const broadcastId = broadcastIdInput.value.trim()
  element<HTMLElement>('broadcast-title').textContent = broadcastId ? `配信 ${broadcastId}` : '配信を見る'
}

function renderLanguages(): void {
  const options = player.languages
  if (languageOptions.childElementCount !== options.length) {
    languageOptions.replaceChildren(
      ...options.map((option) => {
        const button = document.createElement('button')
        button.type = 'button'
        button.className = 'viewer-language'
        button.textContent = option.label
        button.dataset.testid = 'viewer-language-option'
        button.dataset.lang = option.lang
        button.addEventListener('click', () => void player.selectLanguage(option.lang))
        return button
      })
    )
  }
  for (const button of languageOptions.querySelectorAll<HTMLButtonElement>('.viewer-language')) {
    button.setAttribute('aria-pressed', String(button.dataset.lang === player.language))
    button.dataset.pending = String(button.dataset.lang === player.pendingLanguage)
  }
  setStatusText(
    'language-status',
    player.pendingLanguage ? `${player.pendingLanguage} に切り替えています` : `聞いている言語: ${player.language}`
  )
}

function renderStats(): void {
  const stats = player.stats()
  const statsList = element<HTMLElement>('viewer-stats')
  if (stats.playingAudioMicros === undefined) {
    statsList.hidden = true
    return
  }
  const ms = (value: number | undefined) =>
    value === undefined ? '--' : `${value >= 0 ? '+' : '-'}${Math.abs(Math.round(value))} ms`
  const entries: [string, string][] = [
    ['遅延', stats.viewerDelayMs === undefined ? '--' : `${(stats.viewerDelayMs / 1_000).toFixed(2)} s`],
    ['うち届くまで', stats.arrivalDelayMs === undefined ? '--' : `${(stats.arrivalDelayMs / 1_000).toFixed(2)} s`],
    ['うちバッファ', stats.bufferMs === undefined ? '--' : `${(stats.bufferMs / 1_000).toFixed(2)} s`],
    ['映像と音声のずれ', ms(stats.syncOffsetMs)],
    ['訳と原音声のずれ', ms(stats.translationOffsetMs)],
    ['直近の切り替え', stats.lastSwitchSeconds === undefined ? '--' : `${stats.lastSwitchSeconds.toFixed(1)} s`],
    ['遅れて捨てた音声', String(stats.droppedLateAudio)],
    ['遅れて捨てた映像', String(stats.droppedLateVideo)],
    [
      '映像',
      `${stats.frameSize ? `${stats.frameSize.width}x${stats.frameSize.height}` : '--'} ${Math.round(stats.receivedVideoKbps)} kbps`
    ]
  ]
  statsList.replaceChildren(
    ...entries.map(([label, value]) => {
      const item = document.createElement('div')
      const term = document.createElement('dt')
      term.textContent = label
      const detail = document.createElement('dd')
      detail.textContent = value
      item.append(term, detail)
      return item
    })
  )
  statsList.hidden = false
}

/// Sent to the observability server so runs can be compared afterwards.
function logStats(): void {
  const stats = player.stats()
  if (stats.playingAudioMicros === undefined) {
    return
  }
  const defined = (value: number | undefined) => (value === undefined ? -1 : Math.round(value * 1_000) / 1_000)
  telemetry.log('info', 'viewer stats', {
    language: stats.language,
    pending_language: stats.pendingLanguage ?? '',
    playback_delay_ms: player.playbackDelayMs,
    viewer_delay_ms: defined(stats.viewerDelayMs),
    arrival_delay_ms: defined(stats.arrivalDelayMs),
    buffer_ms: defined(stats.bufferMs),
    sync_offset_ms: defined(stats.syncOffsetMs),
    translation_offset_ms: defined(stats.translationOffsetMs),
    last_switch_seconds: defined(stats.lastSwitchSeconds),
    dropped_late_audio: stats.droppedLateAudio,
    dropped_late_video: stats.droppedLateVideo,
    received_video_kbps: Math.round(stats.receivedVideoKbps),
    forwarded_audio_objects: Object.values(stats.forwardedAudioObjects).reduce((sum, count) => sum + count, 0)
  })
}

function appendLog(level: 'info' | 'warn' | 'error', message: string): void {
  const entry = document.createElement('div')
  entry.className = `log-entry log-${level}`
  entry.textContent = `${new Date().toLocaleTimeString()} ${message}`
  element<HTMLDivElement>('logPanel').prepend(entry)
}
