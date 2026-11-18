import av
import numpy as np

JPEG_PIXEL_FORMAT = "yuvj420p"


def picture_to_jpeg(picture: av.VideoFrame, width: int | None = None) -> bytes:
    """Scales the picture to `width`, keeping its aspect ratio, when one is given."""
    if width is None:
        picture = picture.reformat(format=JPEG_PIXEL_FORMAT)
    else:
        height = round(picture.height * width / picture.width / 2) * 2
        picture = picture.reformat(width=width, height=height, format=JPEG_PIXEL_FORMAT)
    encoder = av.CodecContext.create("mjpeg", "w")
    encoder.width = picture.width
    encoder.height = picture.height
    encoder.pix_fmt = JPEG_PIXEL_FORMAT
    return b"".join(bytes(packet) for packet in [*encoder.encode(picture), *encoder.encode(None)])


def rgb_to_jpeg(rgb: bytes, size: tuple[int, int], width: int | None = None) -> bytes:
    """`picture_to_jpeg` for the RGB picture of a pipecat `UserImageRawFrame`."""
    columns, rows = size
    picture = av.VideoFrame.from_ndarray(np.frombuffer(rgb, np.uint8).reshape(rows, columns, 3), format="rgb24")
    return picture_to_jpeg(picture, width)
