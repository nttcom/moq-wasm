import type { Size } from './encodeSize'

/// H.264 levels (ITU-T H.264 Table A-1): the frame size in macroblocks
/// (MaxFS) and the macroblocks a second (MaxMBPS) each allows.
const LEVELS: { idc: number; maxFrameMacroblocks: number; maxMacroblocksPerSecond: number }[] = [
  { idc: 10, maxFrameMacroblocks: 99, maxMacroblocksPerSecond: 1_485 },
  { idc: 11, maxFrameMacroblocks: 396, maxMacroblocksPerSecond: 3_000 },
  { idc: 12, maxFrameMacroblocks: 396, maxMacroblocksPerSecond: 6_000 },
  { idc: 13, maxFrameMacroblocks: 396, maxMacroblocksPerSecond: 11_880 },
  { idc: 20, maxFrameMacroblocks: 396, maxMacroblocksPerSecond: 11_880 },
  { idc: 21, maxFrameMacroblocks: 792, maxMacroblocksPerSecond: 19_800 },
  { idc: 22, maxFrameMacroblocks: 1_620, maxMacroblocksPerSecond: 20_250 },
  { idc: 30, maxFrameMacroblocks: 1_620, maxMacroblocksPerSecond: 40_500 },
  { idc: 31, maxFrameMacroblocks: 3_600, maxMacroblocksPerSecond: 108_000 },
  { idc: 32, maxFrameMacroblocks: 5_120, maxMacroblocksPerSecond: 216_000 },
  { idc: 40, maxFrameMacroblocks: 8_192, maxMacroblocksPerSecond: 245_760 },
  { idc: 41, maxFrameMacroblocks: 8_192, maxMacroblocksPerSecond: 245_760 },
  { idc: 42, maxFrameMacroblocks: 8_704, maxMacroblocksPerSecond: 522_240 },
  { idc: 50, maxFrameMacroblocks: 22_080, maxMacroblocksPerSecond: 589_824 },
  { idc: 51, maxFrameMacroblocks: 36_864, maxMacroblocksPerSecond: 983_040 },
  { idc: 52, maxFrameMacroblocks: 36_864, maxMacroblocksPerSecond: 2_073_600 },
  { idc: 60, maxFrameMacroblocks: 139_264, maxMacroblocksPerSecond: 4_177_920 },
  { idc: 61, maxFrameMacroblocks: 139_264, maxMacroblocksPerSecond: 8_355_840 },
  { idc: 62, maxFrameMacroblocks: 139_264, maxMacroblocksPerSecond: 16_711_680 }
]

const AVC_CODEC = /^(avc[13]\.[0-9a-f]{4})([0-9a-f]{2})$/i

/// `codec` with its level raised to the lowest one that holds `size` at
/// `framerate`, because an encoder rejects a frame larger than its level
/// allows. A level that already holds it, a codec other than H.264, and a size
/// beyond level 6.2 are left as they are.
export function avcCodecForSize(codec: string, size: Size, framerate: number): string {
  const match = AVC_CODEC.exec(codec)
  if (!match) {
    return codec
  }
  const [, profile, levelHex] = match
  const requested = parseInt(levelHex, 16)
  const widthMacroblocks = Math.ceil(size.width / 16)
  const heightMacroblocks = Math.ceil(size.height / 16)
  const frameMacroblocks = widthMacroblocks * heightMacroblocks
  const level = LEVELS.find(
    (level) =>
      level.idc >= requested &&
      frameMacroblocks <= level.maxFrameMacroblocks &&
      frameMacroblocks * framerate <= level.maxMacroblocksPerSecond &&
      // Neither side may exceed sqrt(8 * MaxFS) macroblocks (H.264 A.3.1).
      Math.max(widthMacroblocks, heightMacroblocks) ** 2 <= 8 * level.maxFrameMacroblocks
  )
  if (!level || level.idc === requested) {
    return codec
  }
  return `${profile}${level.idc.toString(16).padStart(2, '0')}`
}
