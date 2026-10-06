// Mirrors the instance schedule of the bot VM: it runs on weekdays 9:00–17:30 Asia/Tokyo.
const TIME_ZONE = 'Asia/Tokyo'
const WEEKDAYS = ['Mon', 'Tue', 'Wed', 'Thu', 'Fri']
const START_MINUTES = 9 * 60
const STOP_MINUTES = 17 * 60 + 30

const OFF_HOURS_NOTICE = '平日日中以外はインスタンスを停止しています'

function botVmRunsAt(now: Date): boolean {
  const parts = new Intl.DateTimeFormat('en-US', {
    timeZone: TIME_ZONE,
    weekday: 'short',
    hour: 'numeric',
    minute: 'numeric',
    hourCycle: 'h23'
  }).formatToParts(now)
  const part = (type: Intl.DateTimeFormatPartTypes) => parts.find((p) => p.type === type)?.value ?? ''
  const minutes = Number(part('hour')) * 60 + Number(part('minute'))
  return WEEKDAYS.includes(part('weekday')) && minutes >= START_MINUTES && minutes < STOP_MINUTES
}

export function showBotOffHoursNotice(element: HTMLElement): void {
  element.textContent = OFF_HOURS_NOTICE
  element.style.display = botVmRunsAt(new Date()) ? 'none' : ''
}
