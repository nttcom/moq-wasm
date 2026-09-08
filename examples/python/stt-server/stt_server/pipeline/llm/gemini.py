from google import genai
from google.genai import types

DEFAULT_SYSTEM_PROMPT = (
    "You are a voice assistant. Answer in the user's language in one or two short "
    "sentences suitable for being read aloud."
)


class GeminiLlm:
    """One Gemini chat per pipeline; the SDK keeps the conversation history."""

    def __init__(
        self,
        api_key: str,
        model: str = "gemini-2.5-flash",
        system_prompt: str = DEFAULT_SYSTEM_PROMPT,
    ) -> None:
        self._chat = genai.Client(api_key=api_key).aio.chats.create(
            model=model,
            config=types.GenerateContentConfig(system_instruction=system_prompt),
        )

    async def reply(self, user_text: str) -> str:
        response = await self._chat.send_message(user_text)
        return response.text or ""
