import base64

from moq_system_one_bot import system_one_client

from moq_camera_detection.prompt import BoxesQuestion, ChoiceQuestion, Prompt, Question

MAX_BOXES = 8

# The choice picked, None when it is not one of the prompt's choices, or each box found as
# [left, top, right, bottom] fractions of the picture.
Answer = str | None | list[list[float]]


def question_id(index: int) -> str:
    return f"q{index}"


def system_one_question(question: Question) -> dict:
    if isinstance(question, BoxesQuestion):
        return {"type": "boxes", "instructions": f"Every {question.target}.", "max": MAX_BOXES}
    return {
        "type": "choice",
        "instructions": question.question,
        "criteria": {choice: None for choice in question.choices},
    }


def read_answer(question: Question, answer: dict) -> Answer:
    if isinstance(question, ChoiceQuestion):
        choice = answer["choice"]
        return choice if choice in question.choices else None
    return [
        [round(found["box"][edge], 3) for edge in ("left", "top", "right", "bottom")] for found in answer["boxes"]
    ]


class SystemOneVisionClient(system_one_client.SystemOneClient):
    async def answer(self, jpeg: bytes, prompt: Prompt) -> list[Answer]:
        """Asks every question of the prompt in one request; the answers follow the questions' order."""
        request = {
            "state": "A camera picture.",
            "images": ["data:image/jpeg;base64," + base64.b64encode(jpeg).decode()],
            "questions": {question_id(index): system_one_question(q) for index, q in enumerate(prompt.questions)},
        }
        answers = await self.ask(request)
        return [read_answer(q, answers[question_id(index)]) for index, q in enumerate(prompt.questions)]
