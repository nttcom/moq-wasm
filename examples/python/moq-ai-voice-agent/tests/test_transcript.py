from moq_ai_voice_agent.transcript import ASSISTANT, transcript_record


def test_a_record_carries_the_role_and_text_of_the_turn():
    # Act
    record = transcript_record(ASSISTANT, "こんにちは", 1_700_000_000_000)

    # Assert: the shape examples/browser/examples/moq-ai-voice-agent/transcript.ts reads
    assert record == {"T": 1_700_000_000_000, "data": {"role": "assistant", "text": "こんにちは"}}
