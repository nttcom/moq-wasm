export type Health = 'ok' | 'warn' | 'bad'

const WARN_LOSS_PERCENT = 1
const BAD_LOSS_PERCENT = 5

export function healthOf(lossPercent: number): Health {
  if (lossPercent < WARN_LOSS_PERCENT) return 'ok'
  return lossPercent <= BAD_LOSS_PERCENT ? 'warn' : 'bad'
}
