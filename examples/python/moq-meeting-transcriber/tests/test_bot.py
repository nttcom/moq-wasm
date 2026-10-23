import argparse

import aiohttp
import pytest

from moq_meeting_transcriber.asr import GRANITE_SPEECH, GraniteSpeechRecognizer, HttpRecognizer
from moq_meeting_transcriber.bot import bot_from_query, http_recognizers
from tests.helpers import NamedRecognizer

ARGS = argparse.Namespace(relay_url="moqt://relay.example", insecure=False)


RECOGNIZERS = {"whisper": NamedRecognizer("whisper"), "qwen3-asr": NamedRecognizer("qwen3-asr")}


@pytest.mark.parametrize(("query", "expected"), [({}, "whisper"), ({"asr": "qwen3-asr"}, "qwen3-asr")])
def test_the_query_picks_the_recognizer_and_whisper_is_the_default(query, expected):
    # Act
    run_bot = bot_from_query(ARGS, RECOGNIZERS, query)

    # Assert
    assert run_bot.args[1].name == expected


def test_a_recognizer_the_bot_was_not_started_with_is_refused():
    # Act / Assert
    with pytest.raises(ValueError, match="unknown asr"):
        bot_from_query(ARGS, RECOGNIZERS, {"asr": "https://elsewhere.example"})


async def test_granite_speech_is_called_through_its_own_recognizer():
    # Arrange
    args = argparse.Namespace(
        asr=[f"{GRANITE_SPEECH}=https://granite.example", "qwen3-asr=https://qwen.example"],
        gcloud_auth=False,
        metadata_auth=False,
    )
    async with aiohttp.ClientSession() as session:
        # Act
        recognizers = http_recognizers(args, session)

    # Assert
    assert type(recognizers[GRANITE_SPEECH]) is GraniteSpeechRecognizer
    assert type(recognizers["qwen3-asr"]) is HttpRecognizer
