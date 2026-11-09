interface Size {
  width: number
  height: number
}

/// The encoder stretches every frame to its configured size, so this keeps the
/// frame's aspect ratio within the requested size. Sizes are even because 4:2:0
/// encoders reject odd ones.
export function fitEncodeSize(requested: Size, frame: Size): Size {
  const scale = Math.min(requested.width / frame.width, requested.height / frame.height)
  return { width: evenFloor(frame.width * scale), height: evenFloor(frame.height * scale) }
}

function evenFloor(value: number): number {
  return Math.floor(value / 2) * 2
}
