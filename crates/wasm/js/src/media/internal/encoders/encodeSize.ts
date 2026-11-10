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

function evenFloor(value: number): number {
  return Math.floor(value / 2) * 2
}
