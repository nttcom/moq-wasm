import { type IncomingSubscribeContext, MoqtClientWrapper, RequestErrorCode } from '@moqt/moqtClient'
import type { MOQTClient, SubgroupObjectMessage } from '../../pkg/moqt_client_wasm'
import { OBJECT_STATUS_END_OF_GROUP } from '../../utils/media/objectStatus'
import { CLOUD_RELAY_PRESETS, LOAD_BALANCED_RELAY_PRESET, configureRelayUrlControls } from '../../utils/relayPresets'
import { type StatusState, getErrorMessage, setStatus } from '../media/common'
import { type Location, type ModerationVerdict, encodeChatRecord, parseModerationVerdicts } from './chatWire'

const CHAT_NAMESPACE = ['anon', 'moq-chat-moderation', 'chat']
const MODERATOR_NAMESPACE = ['anon', 'moq-chat-moderation', 'moderator']
const CHAT_TRACK = 'chat'
const EVENT_TIMELINE_TRACK = 'eventtimeline'
const AUTH_INFO = ''
const SUBGROUP_ID = 0n
const PUBLISHER_PRIORITY = 0
const LARGEST_OBJECT_FILTER = 0x2

class ChatPublisher {
  /// The relay isolates a track whose publisher repeats a location, so a
  /// publisher that comes back must not restart its group ids at zero.
  private nextGroupId = BigInt(Date.now()) * 1_000n

  constructor(
    private readonly client: MOQTClient,
    readonly requestId: bigint,
    private readonly trackAlias: bigint
  ) {}

  /// One group per record: pipecat reads every group from its first object, and a
  /// SUBSCRIBE that joins mid-group starts after it, so each record starts a group.
  reserveLocation(): Location {
    const location = { groupId: this.nextGroupId, objectId: 0n }
    this.nextGroupId += 1n
    return location
  }

  async send(text: string, location: Location): Promise<void> {
    const { groupId, objectId } = location
    await this.client.sendSubgroupHeader(this.trackAlias, groupId, SUBGROUP_ID, PUBLISHER_PRIORITY)
    const payload = encodeChatRecord(text, location)
    await this.client.sendSubgroupObject(this.trackAlias, groupId, SUBGROUP_ID, objectId, undefined, payload, undefined)
    await this.client.sendSubgroupObject(
      this.trackAlias,
      groupId,
      SUBGROUP_ID,
      objectId + 1n,
      OBJECT_STATUS_END_OF_GROUP,
      new Uint8Array(0),
      undefined
    )
  }
}

const session = new MoqtClientWrapper()
const messages = new Map<string, HTMLLIElement>()
let chatPublisher: ChatPublisher | undefined

const urlInput = element<HTMLInputElement>('url')
const joinButton = element<HTMLButtonElement>('joinBtn')
const leaveButton = element<HTMLButtonElement>('leaveBtn')
const messageList = element<HTMLOListElement>('messages')
const chatForm = element<HTMLFormElement>('chatForm')
const chatInput = element<HTMLInputElement>('chatInput')
const sendButton = element<HTMLButtonElement>('sendBtn')

for (const preset of [LOAD_BALANCED_RELAY_PRESET, ...CLOUD_RELAY_PRESETS]) {
  const button = document.createElement('button')
  button.type = 'button'
  button.dataset.url = preset.value
  button.textContent = `Cloud ${preset.label}`
  button.title = preset.helper
  element('urlPresets').appendChild(button)
}
configureRelayUrlControls()

joinButton.addEventListener('click', () => void join())
leaveButton.addEventListener('click', () => void leave())
chatForm.addEventListener('submit', (event) => {
  event.preventDefault()
  void sendChat()
})

async function join(): Promise<void> {
  joinButton.disabled = true
  setStatus('connection-status', '接続中', 'review')
  try {
    await session.connect(urlInput.value.trim())
    session.setOnConnectionClosedHandler(() => resetSession('切断されました', 'error'))
    session.setOnIncomingSubscribeHandler(acceptChatSubscriber)
    session.setOnIncomingUnsubscribeHandler((requestId) => {
      if (chatPublisher?.requestId === requestId) {
        setChatPublisher(undefined)
      }
    })
    session.setOnPublishNamespaceHandler(async ({ publishNamespace, respondOk }) => {
      await respondOk()
      if (sameNamespace(publishNamespace.trackNamespace, MODERATOR_NAMESPACE)) {
        await subscribeVerdicts()
      }
    })
    await session.publishNamespace(CHAT_NAMESPACE, AUTH_INFO)
    await session.subscribeNamespace(MODERATOR_NAMESPACE, AUTH_INFO)
    setStatus('connection-status', '接続済み', 'ok')
    leaveButton.disabled = false
  } catch (error) {
    await session.disconnect()
    resetSession(`接続に失敗しました: ${getErrorMessage(error)}`, 'error')
  }
}

async function leave(): Promise<void> {
  await session.disconnect()
  resetSession('未接続', 'idle')
}

function resetSession(connectionText: string, connectionState: StatusState): void {
  setChatPublisher(undefined)
  setStatus('connection-status', connectionText, connectionState)
  joinButton.disabled = false
  leaveButton.disabled = true
}

async function acceptChatSubscriber({
  subscribe,
  isSuccess,
  code,
  respondOk,
  respondError
}: IncomingSubscribeContext): Promise<void> {
  if (!isSuccess) {
    await respondError(BigInt(code), 'subscribe rejected')
    return
  }
  if (!sameNamespace(subscribe.trackNamespace, CHAT_NAMESPACE) || subscribe.trackName !== CHAT_TRACK) {
    await respondError(RequestErrorCode.TrackDoesNotExist, 'unknown track')
    return
  }
  const trackAlias = await respondOk()
  const client = session.getRawClient()
  if (client) {
    setChatPublisher(new ChatPublisher(client, subscribe.requestId, trackAlias))
  }
}

function setChatPublisher(publisher: ChatPublisher | undefined): void {
  chatPublisher = publisher
  chatInput.disabled = publisher === undefined
  sendButton.disabled = publisher === undefined
  if (publisher) {
    setStatus('moderator-status', 'モデレーター接続済み', 'ok')
  } else {
    setStatus('moderator-status', 'モデレーター未接続', 'idle')
  }
}

async function subscribeVerdicts(): Promise<void> {
  try {
    const { subscribeOk } = await session.subscribe(MODERATOR_NAMESPACE, EVENT_TIMELINE_TRACK, AUTH_INFO, {
      filterType: LARGEST_OBJECT_FILTER,
      forward: true
    })
    session.setOnSubgroupObjectHandler(subscribeOk.trackAlias, (_groupId, object) => applyVerdicts(object))
  } catch (error) {
    console.warn('[moq-chat-moderation] event timeline subscribe failed', error)
  }
}

function applyVerdicts(object: SubgroupObjectMessage): void {
  if (object.objectStatus !== undefined) {
    return
  }
  for (const verdict of parseModerationVerdicts(new Uint8Array(object.objectPayload))) {
    labelMessage(verdict)
  }
}

async function sendChat(): Promise<void> {
  const text = chatInput.value.trim()
  if (!text || !chatPublisher) {
    return
  }
  chatInput.value = ''
  const location = chatPublisher.reserveLocation()
  const item = appendMessage(text, location)
  try {
    await chatPublisher.send(text, location)
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

function labelMessage({ location, abusive }: ModerationVerdict): void {
  const item = messages.get(locationKey(location))
  if (item) {
    setVerdict(item, abusive ? 'abusive' : 'ok', abusive ? '暴言' : '')
  }
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

function sameNamespace(left: string[], right: string[]): boolean {
  return left.join('/') === right.join('/')
}

function element<T extends HTMLElement>(id: string): T {
  const found = document.getElementById(id)
  if (!found) {
    throw new Error(`missing element #${id}`)
  }
  return found as T
}
