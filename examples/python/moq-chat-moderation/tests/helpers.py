from dataclasses import dataclass, field

from moq_chat_moderation.chat import Label


@dataclass
class StubSystemOne:
    probabilities: list[float]
    texts: list[str] = field(default_factory=list)

    async def label_probabilities(self, text: str, labels: tuple[Label, ...]) -> list[float]:
        self.texts.append(text)
        return self.probabilities[: len(labels)]
