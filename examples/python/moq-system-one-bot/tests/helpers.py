import av

from moq_system_one_bot.system_one_status import SystemOneStatus
from moq_system_one_bot.testing import RecordingPublication


def h264_group(frame_count: int, width: int = 64, height: int = 48) -> list[bytes]:
    encoder = av.CodecContext.create("libx264", "w")
    encoder.width = width
    encoder.height = height
    encoder.pix_fmt = "yuv420p"
    encoder.options = {"tune": "zerolatency", "profile": "baseline"}
    pictures = [av.VideoFrame(width, height, "yuv420p") for _ in range(frame_count)]
    for pts, picture in enumerate(pictures):
        picture.pts = pts
    packets = [packet for picture in [*pictures, None] for packet in encoder.encode(picture)]
    return [bytes(packet) for packet in packets]


async def published_status(status: SystemOneStatus) -> RecordingPublication:
    publication = RecordingPublication()
    status.publish_on(publication)
    await publication.connect()
    return publication


async def next_status(publication: RecordingPublication) -> str:
    _, status = await publication.next_write()
    return status["system_one"]
