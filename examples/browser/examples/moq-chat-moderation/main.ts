import { MoqtClientWrapper } from '@moqt/moqtClient'
import { pageRoomId } from '../../utils/botRoom'
import { SingleObjectGroupPublisher, setUpSystemOneBotPage } from '../../utils/systemOneBot'
import { element, setStatus } from '../../utils/page'
import {
  type Label,
  type LabelProbability,
  type Location,
  MAX_LABELS,
  MAX_LABEL_DESCRIPTION_CHARS,
  MAX_LABEL_NAME_CHARS,
  type ModerationVerdict,
  encodeChatMessage,
  parseModerationVerdicts
} from './chatWire'

const ROOM = pageRoomId()
const CHAT_NAMESPACE = ['anon', 'moq-chat-moderation', ROOM, 'chat']
const MODERATOR_NAMESPACE = ['anon', 'moq-chat-moderation', ROOM, 'moderator']
const BOT_SERVICE = {
  param: 'botUrl',
  local: 'http://127.0.0.1:8097',
  cloud: 'https://moq-chat-moderation.moqt.research.skyway.io'
}
const CHAT_TRACK = 'chat'
const REMOVED_MESSAGE_TEXT = 'モデレーターによって削除されました'
const LABEL_THRESHOLD = 0.7
const ABUSIVE_LABEL = '暴言'
const DEFAULT_LABELS: Label[] = [
  { name: '喜', description: '喜んでいる、うれしい' },
  { name: '怒', description: '怒っている、腹を立てている' },
  { name: '哀', description: '悲しんでいる、落ち込んでいる' },
  { name: '楽', description: '楽しんでいる、愉快' },
  { name: ABUSIVE_LABEL, description: '相手を侮辱・攻撃・脅す' },
  { name: 'スパム', description: '宣伝・勧誘・無関係な繰り返し' }
]

/// `labels` is undefined until the message is judged.
type ChatEntry = { item: HTMLLIElement; labels?: LabelProbability[] }

const session = new MoqtClientWrapper()
const messages = new Map<string, ChatEntry>()
let chatPublisher: SingleObjectGroupPublisher | undefined
let sidebarEntry: ChatEntry | undefined

const messageList = element<HTMLOListElement>('messages')
const chatForm = element<HTMLFormElement>('chatForm')
const chatInput = element<HTMLInputElement>('chatInput')
const sendButton = element<HTMLButtonElement>('sendBtn')
const labelList = element<HTMLDivElement>('labels')
const addLabelButton = element<HTMLButtonElement>('addLabelBtn')
const sidebar = element<HTMLElement>('sidebar')
const sidebarText = element<HTMLParagraphElement>('sidebarText')
const sidebarState = element<HTMLParagraphElement>('sidebarState')
const sidebarLabels = element<HTMLOListElement>('sidebarLabels')

setUpSystemOneBotPage({
  session,
  logTag: 'moq-chat-moderation',
  ownNamespace: CHAT_NAMESPACE,
  ownTracks: [CHAT_TRACK],
  botNamespace: MODERATOR_NAMESPACE,
  room: ROOM,
  botService: BOT_SERVICE,
  onSubscriber: ({ requestId, trackAlias }) =>
    setChatPublisher(new SingleObjectGroupPublisher(session, requestId, trackAlias)),
  onUnsubscribe: (requestId) => {
    if (chatPublisher?.requestId === requestId) {
      setChatPublisher(undefined)
    }
  },
  onTimelineObject: (payload) => parseModerationVerdicts(payload).forEach(moderateMessage),
  onReset: () => setChatPublisher(undefined)
})

DEFAULT_LABELS.forEach(addLabelRow)
chatForm.addEventListener('submit', (event) => {
  event.preventDefault()
  void sendChat()
})
addLabelButton.addEventListener('click', () => addLabelRow({ name: '', description: '' }).focus())
labelList.addEventListener('input', updateLabelRows)
element<HTMLButtonElement>('closeSidebarBtn').addEventListener('click', () => showSidebar(undefined))

function addLabelRow({ name, description }: Label): HTMLInputElement {
  const row = document.createElement('div')
  row.className = 'label-row'
  const nameInput = labelInput(name, MAX_LABEL_NAME_CHARS, 'ラベル名')
  nameInput.required = true
  nameInput.pattern = '.*\\S.*'
  nameInput.classList.add('label-name')
  nameInput.dataset.testid = 'chat-moderation-label-name-input'
  const descriptionInput = labelInput(description, MAX_LABEL_DESCRIPTION_CHARS, 'ラベルの説明')
  const remove = document.createElement('button')
  remove.type = 'button'
  remove.textContent = '削除'
  remove.dataset.testid = 'chat-moderation-remove-label-button'
  remove.addEventListener('click', () => {
    row.remove()
    updateLabelRows()
  })
  row.append(nameInput, descriptionInput, remove)
  labelList.append(row)
  updateLabelRows()
  return nameInput
}

function labelInput(value: string, maxLength: number, ariaLabel: string): HTMLInputElement {
  const input = document.createElement('input')
  input.type = 'text'
  input.value = value
  input.maxLength = maxLength
  input.autocomplete = 'off'
  // The form attribute makes sending validate the label inputs like the message.
  input.setAttribute('form', chatForm.id)
  input.setAttribute('aria-label', ariaLabel)
  return input
}

/// Verdicts name a label by its name, so two labels may not share one.
function updateLabelRows(): void {
  const names = new Set<string>()
  labelList.querySelectorAll<HTMLInputElement>('.label-name').forEach((input) => {
    const name = input.value.trim()
    input.setCustomValidity(names.has(name) ? '同じ名前のラベルがあります' : '')
    names.add(name)
  })
  addLabelButton.disabled = labelList.children.length >= MAX_LABELS
}

function currentLabels(): Label[] {
  return [...labelList.children].map((row) => {
    const [name, description] = [...row.querySelectorAll('input')].map((input) => input.value.trim())
    return { name, description }
  })
}

function setChatPublisher(publisher: SingleObjectGroupPublisher | undefined): void {
  chatPublisher = publisher
  chatInput.disabled = publisher === undefined
  sendButton.disabled = publisher === undefined
  if (publisher) {
    setStatus('moderator-status', 'モデレーター接続済み', 'ok')
  } else {
    setStatus('moderator-status', 'モデレーター未接続', 'idle')
  }
}

async function sendChat(): Promise<void> {
  const text = chatInput.value.trim()
  if (!text || !chatPublisher) {
    return
  }
  chatInput.value = ''
  const labels = currentLabels()
  const location = { groupId: chatPublisher.reserveGroupId(), objectId: 0n }
  const entry = appendMessage(text, location)
  // The moderator does not judge a message without labels.
  if (labels.length === 0) {
    moderateMessage({ location, labels: [] })
  }
  try {
    await chatPublisher.send(encodeChatMessage(text, labels), location.groupId)
  } catch (error) {
    console.error('[moq-chat-moderation] chat send failed', error)
    setVerdict(entry.item, 'failed', '送信失敗')
  }
}

function appendMessage(text: string, location: Location): ChatEntry {
  const item = document.createElement('li')
  item.className = 'chat-message'
  item.dataset.verdict = 'pending'
  const body = document.createElement('span')
  body.className = 'chat-text'
  body.textContent = text
  const chips = document.createElement('span')
  chips.className = 'chat-chips'
  const label = document.createElement('span')
  label.className = 'chat-label'
  label.textContent = '判定中'
  item.append(body, chips, label)
  messageList.append(item)
  messageList.scrollTop = messageList.scrollHeight
  const entry: ChatEntry = { item }
  item.addEventListener('click', () => showSidebar(entry))
  messages.set(locationKey(location), entry)
  return entry
}

function moderateMessage({ location, labels }: ModerationVerdict): void {
  const entry = messages.get(locationKey(location))
  if (!entry) {
    return
  }
  entry.labels = labels
  const given = labels.filter(({ probability }) => probability >= LABEL_THRESHOLD)
  const chips = entry.item.querySelector('.chat-chips')
  chips?.replaceChildren(
    ...given.map(({ name }) => {
      const chip = document.createElement('span')
      chip.className = 'chat-chip'
      chip.textContent = name
      return chip
    })
  )
  const abusive = given.some(({ name }) => name === ABUSIVE_LABEL)
  if (abusive) {
    const body = entry.item.querySelector('.chat-text')
    if (body) {
      body.textContent = REMOVED_MESSAGE_TEXT
    }
  }
  setVerdict(entry.item, abusive ? 'removed' : 'ok', '')
  if (sidebarEntry === entry) {
    showSidebar(entry)
  }
}

function showSidebar(entry: ChatEntry | undefined): void {
  sidebarEntry = entry
  sidebar.hidden = entry === undefined
  if (!entry) {
    return
  }
  sidebarText.textContent = entry.item.querySelector('.chat-text')?.textContent ?? ''
  const labels = [...(entry.labels ?? [])].sort((a, b) => b.probability - a.probability)
  sidebarState.textContent = entry.labels === undefined ? '判定中' : labels.length === 0 ? 'ラベルなし' : ''
  sidebarLabels.replaceChildren(
    ...labels.map(({ name, probability }) => {
      const row = document.createElement('li')
      row.className = 'label-score'
      row.dataset.testid = 'chat-moderation-label-score'
      const nameText = document.createElement('span')
      nameText.textContent = name
      const percent = document.createElement('span')
      percent.textContent = `${Math.round(probability * 100)}%`
      const bar = document.createElement('meter')
      bar.value = probability
      bar.setAttribute('aria-label', name)
      row.append(nameText, percent, bar)
      return row
    })
  )
}

function setVerdict(item: HTMLLIElement, verdict: string, labelText: string): void {
  item.dataset.verdict = verdict
  const label = item.querySelector('.chat-label')
  if (label) {
    label.textContent = labelText
  }
}

function locationKey({ groupId, objectId }: Location): string {
  return `${groupId}:${objectId}`
}
