import aiohttp
from moq_system_one_bot.system_one_status import SystemOneStatus
from moq_bot_rooms.identity_token import IdentityToken

from moq_chat_moderation.chat import Label


def question_id(label: int) -> str:
    return f"l{label}"


def label_question(label: Label) -> dict:
    # Asked in Japanese: on Japanese chat and labels, d1-2B separated spam from ordinary chat better than
    # with the same question in English.
    meaning = f"（{label.description}）" if label.description else ""
    return {"type": "noul", "instructions": f"このチャットメッセージは「{label.name}」{meaning}に当てはまりますか？"}


class SystemOneClient:
    def __init__(
        self,
        session: aiohttp.ClientSession,
        url: str,
        status: SystemOneStatus,
        identity_token: IdentityToken | None = None,
    ):
        self._session = session
        self._url = url
        self._status = status
        self._identity_token = identity_token

    async def label_probabilities(self, text: str, labels: tuple[Label, ...]) -> list[float]:
        """Probabilities in the order of `labels`."""
        request = {
            "state": text,
            "questions": {question_id(index): label_question(label) for index, label in enumerate(labels)},
        }
        headers = (
            {"Authorization": f"Bearer {await self._identity_token.value()}"}
            if self._identity_token
            else {}
        )
        async with (
            self._status.request(),
            self._session.post(self._url, json=request, headers=headers) as response,
        ):
            response.raise_for_status()
            answers = (await response.json())["answers"]
        return [float(answers[question_id(index)]["noul"]) for index in range(len(labels))]
