import aiohttp

JEV_URL = "https://api.typesafe.ai/v1/systemone"
JEV_MODEL = "jev-latest"
ABUSIVE_QUESTION_ID = "abusive"
ABUSIVE_QUESTION = {
    "type": "noul",
    "instructions": "Is this chat message abusive?",
    "criteria": {
        "true": "Insults, harasses, threatens or demeans someone",
        "false": "Ordinary conversation, including mild disagreement",
    },
}


class JevClient:
    def __init__(self, session: aiohttp.ClientSession, api_key: str, url: str = JEV_URL):
        self._session = session
        self._api_key = api_key
        self._url = url

    async def abusive_probability(self, text: str) -> float:
        request = {
            "model": JEV_MODEL,
            "state": text,
            "questions": {ABUSIVE_QUESTION_ID: ABUSIVE_QUESTION},
        }
        headers = {"Authorization": f"Bearer {self._api_key}"}
        async with self._session.post(self._url, json=request, headers=headers) as response:
            response.raise_for_status()
            body = await response.json()
        return float(body["answers"][ABUSIVE_QUESTION_ID]["noul"])
