import av

JPEG_PIXEL_FORMAT = "yuvj420p"


def keyframe_to_jpeg(annexb: bytes) -> bytes | None:
    decoder = av.CodecContext.create("h264", "r")
    try:
        pictures = [*decoder.decode(av.Packet(annexb)), *decoder.decode(None)]
    except av.FFmpegError:
        return None
    if not pictures:
        return None
    picture = pictures[-1].reformat(format=JPEG_PIXEL_FORMAT)
    encoder = av.CodecContext.create("mjpeg", "w")
    encoder.width = picture.width
    encoder.height = picture.height
    encoder.pix_fmt = JPEG_PIXEL_FORMAT
    return b"".join(bytes(packet) for packet in [*encoder.encode(picture), *encoder.encode(None)])
