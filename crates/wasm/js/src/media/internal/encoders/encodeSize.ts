export interface Size {
  width: number
  height: number
}

/// The encoder stretches every frame to its configured size, so this keeps the
/// frame's aspect ratio within the requested size, after dividing it by
/// `scaleResolutionDownBy`, without scaling the frame up. Sizes are even
/// because 4:2:0 encoders reject odd ones.
export function fitEncodeSize(requested: Partial<Size> & { scaleResolutionDownBy?: number }, frame: Size): Size {
  const scale = Math.min(
    1,
    1 / (requested.scaleResolutionDownBy ?? 1),
    (requested.width ?? Number.POSITIVE_INFINITY) / frame.width,
    (requested.height ?? Number.POSITIVE_INFINITY) / frame.height
  )
  return { width: evenFloor(frame.width * scale), height: evenFloor(frame.height * scale) }
}

/// Long side by short side, tried in order when an encoder rejects a frame's
/// size. Hardware H.264 encoders commonly stop near 4096x2304, below what
/// H.264 level 6 allows.
const FALLBACK_BOUNDS: readonly Size[] = [
  { width: 3840, height: 2160 },
  { width: 1920, height: 1080 }
]

/// `size` scaled down, keeping its aspect ratio, so that its long side and its
/// short side fit the long side and the short side of `bounds`.
export function fitWithinBounds(size: Size, bounds: Size): Size {
  const box = size.height > size.width ? { width: bounds.height, height: bounds.width } : bounds
  return fitEncodeSize(box, size)
}

/// `undefined` (the frame's own size) first, then each `FALLBACK_BOUNDS` entry
/// that makes `size` smaller than the attempt before.
export function fallbackBounds(size: Size): (Size | undefined)[] {
  const attempts: (Size | undefined)[] = [undefined]
  let width = size.width
  for (const bounds of FALLBACK_BOUNDS) {
    const fitted = fitWithinBounds(size, bounds)
    if (fitted.width < width) {
      attempts.push(bounds)
      width = fitted.width
    }
  }
  return attempts
}

function evenFloor(value: number): number {
  return Math.floor(value / 2) * 2
}
