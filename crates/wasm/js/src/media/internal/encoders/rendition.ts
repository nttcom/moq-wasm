export type Rendition = {
  name: string
  label: string
  /// Bits per second.
  bitrate: number
  /// Like `MainVideo.framerate`; the main video's by default.
  framerate?: number
} & RenditionSize

/// A rendition's size, either bounded or scaled from the captured frame.
export type RenditionSize =
  | {
      /// Bound the encoded size like `MainVideo.width` and `height`.
      width: number
      height: number
      scaleResolutionDownBy?: never
    }
  | {
      /// Divides the captured frame's width and height, as in WebRTC: 4 encodes
      /// a 1280x720 camera at 320x180 whatever `main` bounds. At least 1.
      scaleResolutionDownBy: number
      width?: never
      height?: never
    }

/// Throws a `RangeError` for a `scaleResolutionDownBy` below 1, which would
/// scale the frame up, or not finite.
export function checkRenditions(renditions: Rendition[] = []): void {
  for (const { name, scaleResolutionDownBy } of renditions) {
    if (
      scaleResolutionDownBy !== undefined &&
      !(Number.isFinite(scaleResolutionDownBy) && scaleResolutionDownBy >= 1)
    ) {
      throw new RangeError(`rendition ${name}: scaleResolutionDownBy must be at least 1, not ${scaleResolutionDownBy}`)
    }
  }
}
