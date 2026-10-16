import { MoqtClientWrapper } from '@moqt/moqtClient'
import { SingleObjectGroupPublisher, setUpD3FlashBotPage } from '../../utils/d3FlashBot'
import { element, setStatus } from '../../utils/page'
import {
  type Location,
  MAX_CHAT_TEXT_LENGTH,
  type ModerationVerdict,
  encodeChatRecord,
  parseModerationVerdicts
} from './chatWire'

const CHAT_NAMESPACE = ['anon', 'moq-chat-moderation', 'chat']
const MODERATOR_NAMESPACE = ['anon', 'moq-chat-moderation', 'moderator']
const CHAT_TRACK = 'chat'
const REMOVED_MESSAGE_TEXT = 'モデレーターによって削除されました'

const session = new MoqtClientWrapper()
const messages = new Map<string, HTMLLIElement>()
let chatPublisher: SingleObjectGroupPublisher | undefined

const messageList = element<HTMLOListElement>('messages')
const chatForm = element<HTMLFormElement>('chatForm')
const chatInput = element<HTMLInputElement>('chatInput')
const sendButton = element<HTMLButtonElement>('sendBtn')

chatInput.maxLength = MAX_CHAT_TEXT_LENGTH

setUpD3FlashBotPage({
  session,
  logTag: 'moq-chat-moderation',
  ownNamespace: CHAT_NAMESPACE,
  ownTracks: [CHAT_TRACK],
  botNamespace: MODERATOR_NAMESPACE,
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

chatForm.addEventListener('submit', (event) => {
  event.preventDefault()
  void sendChat()
})

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
  const location = { groupId: chatPublisher.reserveGroupId(), objectId: 0n }
  const item = appendMessage(text, location)
  try {
    await chatPublisher.send(encodeChatRecord(text, location), location.groupId)
  } catch (error) {
    console.error('[moq-chat-moderation] chat send failed', error)
    setVerdict(item, 'failed', '送信失敗')
  }
}

function appendMessage(text: string, location: Location): HTMLLIElement {
  const item = document.createElement('li')
  item.className = 'chat-message'
  item.dataset.verdict = 'pending'
  const body = document.createElement('span')
  body.className = 'chat-text'
  body.textContent = text
  const label = document.createElement('span')
  label.className = 'chat-label'
  label.textContent = '判定中'
  item.append(body, label)
  messageList.append(item)
  messageList.scrollTop = messageList.scrollHeight
  messages.set(locationKey(location), item)
  return item
}

function moderateMessage({ location, abusive }: ModerationVerdict): void {
  const item = messages.get(locationKey(location))
  if (!item) {
    return
  }
  if (abusive) {
    const body = item.querySelector('.chat-text')
    if (body) {
      body.textContent = REMOVED_MESSAGE_TEXT
    }
  }
  setVerdict(item, abusive ? 'removed' : 'ok', '')
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
