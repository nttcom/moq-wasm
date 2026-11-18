/// The recording the Meeting shows for the room's recorder bot (bots/roomBots.ts).

import type { RoomBot } from '../bots/roomBots'

export const RECORDER_BOT_TYPE = 'recorder'

/// A recording the room's recorder bot is making, as the header and the controls show it.
export interface ActiveRecording {
  id: string
  state: 'starting' | 'recording' | 'stopping'
}

export function activeRecording(bot: RoomBot | undefined): ActiveRecording | undefined {
  switch (bot?.state) {
    case 'starting':
    case 'stopping':
      return { id: bot.id, state: bot.state }
    case 'running':
      return { id: bot.id, state: 'recording' }
    default:
      return undefined
  }
}
