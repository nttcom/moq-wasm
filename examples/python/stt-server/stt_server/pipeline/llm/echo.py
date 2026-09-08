class EchoLlm:
    """Replies with the transcript itself; lets the reply path be exercised
    without a model or credentials."""

    async def reply(self, user_text: str) -> str:
        return user_text
