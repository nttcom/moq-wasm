import type { Browser, BrowserContext, Locator, Page } from '@playwright/test'
import { CALL_INDEX_PATH } from '../playwright.helpers'

export const RELAY_A_URL = 'https://127.0.0.1:4433'
export const RELAY_B_URL = 'https://127.0.0.1:4434'

export interface CallClientPageModel {
  page: Page
  roomNameInput: Locator
  userNameInput: Locator
  relayARadio: Locator
  relayBRadio: Locator
  submitButton: Locator
  roomName: Locator
  leaveButton: Locator
  toggleCameraButton: Locator
  toggleMicrophoneButton: Locator
  chatInput: Locator
  chatSendButton: Locator
}

export interface CallE2EClient {
  context: BrowserContext
  page: CallClientPageModel
}

// Use a short camera keyframe interval (~1s at 30fps) so newly-subscribed clients
// get a renderable keyframe quickly even when several streams decode concurrently.
const KEYFRAME_INTERVAL = 30

async function openCallPage(page: Page): Promise<void> {
  await page.goto(`${CALL_INDEX_PATH}?keyframeInterval=${KEYFRAME_INTERVAL}`, { waitUntil: 'domcontentloaded' })
}

function createCallClientPageModel(page: Page): CallClientPageModel {
  return {
    page,
    roomNameInput: page.getByTestId('join-room-name-input'),
    userNameInput: page.getByTestId('join-user-name-input'),
    relayARadio: page.getByTestId('join-relay-a-radio'),
    relayBRadio: page.getByTestId('join-relay-b-radio'),
    submitButton: page.getByTestId('join-submit-button'),
    roomName: page.getByTestId('room-name'),
    leaveButton: page.getByTestId('room-leave-button'),
    toggleCameraButton: page.getByTestId('toggle-camera-button'),
    toggleMicrophoneButton: page.getByTestId('toggle-microphone-button'),
    chatInput: page.getByTestId('chat-message-input'),
    chatSendButton: page.getByTestId('chat-send-button')
  }
}

export async function arrangeCallClient(browser: Browser): Promise<CallE2EClient> {
  const context = await browser.newContext({
    ignoreHTTPSErrors: true,
    permissions: ['camera', 'microphone']
  })
  const rawPage = await context.newPage()
  await openCallPage(rawPage)
  return {
    context,
    page: createCallClientPageModel(rawPage)
  }
}

// Join room and pick a relay. Relay 'a' = 4433, 'b' = 4434.
export async function joinRoom(
  model: CallClientPageModel,
  roomName: string,
  userName: string,
  relay: 'a' | 'b'
): Promise<void> {
  await model.roomNameInput.fill(roomName)
  await model.userNameInput.fill(userName)
  if (relay === 'b') {
    await model.relayBRadio.click()
  } else {
    await model.relayARadio.click()
  }
  await model.submitButton.click()
}

// Enable camera and mic so the client publishes video+audio.
export async function enableMedia(model: CallClientPageModel): Promise<void> {
  await model.toggleCameraButton.click()
  await model.toggleMicrophoneButton.click()
}

// Locate the member card for a remote member by name.
export function getMemberCard(page: Page, memberName: string): Locator {
  return page.getByTestId(`member-card-${memberName}`)
}

// Locate the catalog status span inside a member card.
export function getCatalogStatus(page: Page, memberName: string): Locator {
  return page.getByTestId(`catalog-status-${memberName}`)
}

// Locate the video element inside a remote member card.
export function getMemberVideo(page: Page, memberName: string): Locator {
  return page.getByTestId(`member-video-${memberName}`)
}

// Locate the audio element inside a remote member card.
export function getMemberAudio(page: Page, memberName: string): Locator {
  return page.getByTestId(`member-audio-${memberName}`)
}

// Locate the subscribe-video button for a remote member.
export function getSubscribeVideoButton(page: Page, memberName: string): Locator {
  return page.getByTestId(`subscribe-video-button-${memberName}`)
}

// Locate the subscribe-audio button for a remote member.
export function getSubscribeAudioButton(page: Page, memberName: string): Locator {
  return page.getByTestId(`subscribe-audio-button-${memberName}`)
}

// Locate a remote member's per-role subscription status (e.g. 'Subscribed').
export function getTrackStatus(
  page: Page,
  role: 'video' | 'audio' | 'chat' | 'screenshare',
  memberName: string
): Locator {
  return page.getByTestId(`track-status-${role}-${memberName}`)
}

// Send a chat message from this client.
export async function sendChatMessage(model: CallClientPageModel, text: string): Promise<void> {
  await model.chatInput.fill(text)
  await model.chatSendButton.click()
}

// Locate a chat message bubble by its text content.
export function getChatMessageByText(page: Page, text: string): Locator {
  return page.getByTestId('chat-message').filter({ hasText: text })
}
