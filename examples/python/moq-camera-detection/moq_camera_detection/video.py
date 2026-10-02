import av

JPEG_PIXEL_FORMAT = "yuvj420p"


class GroupDecoder:
    def __init__(self):
        self._decoder = av.CodecContext.create("h264", "r")

    def decode(self, annexb: bytes) -> av.VideoFrame | None:
        try:
            pictures = self._decoder.decode(av.Packet(annexb))
        except av.FFmpegError:
            return None
        return pictures[-1] if pictures else None


def picture_to_jpeg(picture: av.VideoFrame) -> bytes:
    picture = picture.reformat(format=JPEG_PIXEL_FORMAT)
    encoder = av.CodecContext.create("mjpeg", "w")
    encoder.width = picture.width
    encoder.height = picture.height
    encoder.pix_fmt = JPEG_PIXEL_FORMAT
    return b"".join(bytes(packet) for packet in [*encoder.encode(picture), *encoder.encode(None)])
