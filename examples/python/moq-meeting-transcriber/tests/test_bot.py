import argparse

import aiohttp
import pytest

from moq_meeting_transcriber.asr import GRANITE_SPEECH, GraniteSpeechRecognizer, HttpRecognizer
from moq_meeting_transcriber.bot import recognizer_named
from moq_meeting_transcriber.granite import SharedGranite

ARGS = argparse.Namespace(
    asr=[f"{GRANITE_SPEECH}=https://granite.example", "qwen3-asr=https://qwen.example"],
    gcloud_auth=False,
    metadata_auth=False,
)


@pytest.mark.parametrize(
    ("name", "expected"),
    [
        ("granite-speech-5-470m", SharedGranite),
        (GRANITE_SPEECH, GraniteSpeechRecognizer),
        ("qwen3-asr", HttpRecognizer),
    ],
)
async def test_the_option_picks_the_recognizer(name, expected):
    async with aiohttp.ClientSession() as http:
        # Act
        recognizer = recognizer_named(ARGS, http, name)

    # Assert
    assert type(recognizer) is expected
    assert recognizer.name == name


async def test_a_recognizer_the_bot_was_not_started_with_is_refused():
    async with aiohttp.ClientSession() as http:
        # Act / Assert
        with pytest.raises(ValueError, match="unknown asr"):
            recognizer_named(ARGS, http, "https://elsewhere.example")
