export const RANGES = {
  '1m': 60_000,
  '5m': 5 * 60_000,
  '15m': 15 * 60_000,
  '1h': 3_600_000,
  '6h': 6 * 3_600_000,
  '1d': 86_400_000,
  '7d': 7 * 86_400_000
} as const

export type RangeName = keyof typeof RANGES

export const RANGE_NAMES = Object.keys(RANGES) as RangeName[]
