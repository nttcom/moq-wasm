from google import genai
from google.genai import types

DEFAULT_SYSTEM_PROMPT = (
    "You are a voice assistant. Answer in the user's language in one or two short "
    "sentences suitable for being read aloud."
)


class GeminiLlm:
    """One Gemini chat per pipeline. The SDK reads GEMINI_API_KEY (or
    GOOGLE_API_KEY) from the environment and keeps the conversation history."""

    def __init__(self, model: str, system_prompt: str) -> None:
        # The client owns the HTTP transport and closes it when collected, so
        # it has to outlive the chat.
        self._client = genai.Client()
        self._chat = self._client.aio.chats.create(
            model=model,
            config=types.GenerateContentConfig(system_instruction=system_prompt),
        )

    async def reply(self, user_text: str) -> str:
        response = await self._chat.send_message(user_text)
        return response.text or ""
