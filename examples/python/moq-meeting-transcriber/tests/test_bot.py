import argparse

import pytest

from moq_meeting_transcriber.bot import bot_from_query
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
