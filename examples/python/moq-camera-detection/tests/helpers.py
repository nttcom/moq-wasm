from dataclasses import dataclass, field

from moq_camera_detection.prompt import ChoiceQuestion, Prompt
from moq_camera_detection.vision import Answer


YES_NO_PROMPT = Prompt((ChoiceQuestion("Is there a person?", ("yes", "no")),), (7, 0))


@dataclass
class StubVision:
    answers: list[Answer]
    images: list[bytes] = field(default_factory=list)

    async def answer(self, jpeg: bytes, prompt: Prompt) -> list[Answer]:
        self.images.append(jpeg)
        return self.answers
