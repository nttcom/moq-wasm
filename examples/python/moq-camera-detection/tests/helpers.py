from dataclasses import dataclass, field

import av

from moq_camera_detection.prompt import ChoiceQuestion, Prompt
from moq_camera_detection.vision import Answer


def h264_group(frame_count: int) -> list[bytes]:
    encoder = av.CodecContext.create("libx264", "w")
    encoder.width = 64
    encoder.height = 48
    encoder.pix_fmt = "yuv420p"
    encoder.options = {"tune": "zerolatency", "profile": "baseline"}
    pictures = [av.VideoFrame(64, 48, "yuv420p") for _ in range(frame_count)]
    for pts, picture in enumerate(pictures):
        picture.pts = pts
    packets = [packet for picture in [*pictures, None] for packet in encoder.encode(picture)]
    return [bytes(packet) for packet in packets]


YES_NO_PROMPT = Prompt((ChoiceQuestion("Is there a person?", ("yes", "no")),), (7, 0))


@dataclass
class StubVision:
    answers: list[Answer]
    images: list[bytes] = field(default_factory=list)

    async def answer(self, jpeg: bytes, prompt: Prompt) -> list[Answer]:
        self.images.append(jpeg)
        return self.answers
