from moq_system_one_bot import system_one_client

from moq_chat_moderation.chat import Label


def question_id(label: int) -> str:
    return f"l{label}"


def label_question(label: Label) -> dict:
    # Asked in Japanese: on Japanese chat and labels, d1-2B separated spam from ordinary chat better than
    # with the same question in English.
    meaning = f"（{label.description}）" if label.description else ""
    return {"type": "noul", "instructions": f"このチャットメッセージは「{label.name}」{meaning}に当てはまりますか？"}


class SystemOneClient(system_one_client.SystemOneClient):
    async def label_probabilities(self, text: str, labels: tuple[Label, ...]) -> list[float]:
        """Probabilities in the order of `labels`."""
        request = {
            "state": text,
            "questions": {question_id(index): label_question(label) for index, label in enumerate(labels)},
        }
        answers = await self.ask(request)
        return [float(answers[question_id(index)]["noul"]) for index in range(len(labels))]
