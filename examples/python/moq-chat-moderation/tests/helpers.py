from dataclasses import dataclass, field
from typing import Any

from aiohttp import web

from moq_chat_moderation.chat import Label


@dataclass
class StubSystemOne:
    probabilities: list[float]
    texts: list[str] = field(default_factory=list)

    async def label_probabilities(self, text: str, labels: tuple[Label, ...]) -> list[float]:
        self.texts.append(text)
        return self.probabilities[: len(labels)]


@dataclass
class FakeSystemOneServer:
    nouls: list[float]
    url: str = ""
    requests: list[tuple[str | None, dict[str, Any]]] = field(default_factory=list)

    async def handle(self, request: web.Request) -> web.Response:
        body = await request.json()
        self.requests.append((request.headers.get("Authorization"), body))
        answers = {question: {"type": "noul", "noul": noul} for question, noul in zip(body["questions"], self.nouls)}
        return web.json_response({"answers": answers})
