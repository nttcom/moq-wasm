import asyncio
import math
import struct

import numpy as np

from stt_server.stt.segmenter import SpeechSegmenter
from stt_server.stt.whisper_local import RecognizedSegment, WhisperLocalBackend

SAMPLE_RATE = 16000


def tone(seconds: float, amplitude: int = 8000) -> bytes:
    return b"".join(
        struct.pack("<h", int(amplitude * math.sin(2 * math.pi * 300 * i / SAMPLE_RATE)))
        for i in range(int(seconds * SAMPLE_RATE))
    )


def silence(seconds: float) -> bytes:
    return bytes(2 * int(seconds * SAMPLE_RATE))


def test_segment_ends_after_a_pause():
    # Arrange
    segmenter = SpeechSegmenter(SAMPLE_RATE, min_sec=1.0, silence_sec=0.4)

    # Act
    segments = segmenter.push(tone(1.5) + silence(0.5))

    # Assert
    assert len(segments) == 1
    assert 1.5 * SAMPLE_RATE <= len(segments[0]) <= 2.0 * SAMPLE_RATE


def test_continuous_speech_is_cut_at_max_length():
    # Arrange
    segmenter = SpeechSegmenter(SAMPLE_RATE, max_sec=2.0)

    # Act
    segments = segmenter.push(tone(5.0))

    # Assert
    assert [len(segment) for segment in segments] == [2 * SAMPLE_RATE, 2 * SAMPLE_RATE]
    assert len(segmenter.flush()) == SAMPLE_RATE


def test_flush_drops_a_buffer_shorter_than_one_frame():
    # Arrange
    segmenter = SpeechSegmenter(SAMPLE_RATE)
    segmenter.push(tone(0.01))

    # Act / Assert
    assert segmenter.flush() is None


async def test_backend_transcribes_each_segment_off_the_reader_path():
    # Arrange
    seen: list[np.ndarray] = []

    def fake_transcribe(audio: np.ndarray) -> list[RecognizedSegment]:
        seen.append(audio)
        return [RecognizedSegment(f"segment {len(seen)}", no_speech_prob=0.1)]

    transcripts = []
    backend = WhisperLocalBackend(transcribe=fake_transcribe, max_segment_sec=1.0)
    await backend.start(transcripts.append)

    # Act
    await backend.send_pcm(tone(2.5))
    await backend.close()

    # Assert
    assert [transcript.text for transcript in transcripts] == ["segment 1", "segment 2", "segment 3"]
    assert all(transcript.is_final for transcript in transcripts)
    assert seen[0].dtype == np.float32 and abs(seen[0]).max() <= 1.0


def test_silence_only_buffers_are_discarded():
    # Arrange
    segmenter = SpeechSegmenter(SAMPLE_RATE, min_sec=1.0, max_sec=2.0)

    # Act
    segments = segmenter.push(silence(5.0))

    # Assert
    assert segments == []
    assert segmenter.flush() is None


def test_speech_followed_by_long_silence_yields_only_the_speech():
    # Arrange
    segmenter = SpeechSegmenter(SAMPLE_RATE, min_sec=1.0, max_sec=2.0)

    # Act
    segments = segmenter.push(tone(1.2) + silence(6.0))

    # Assert
    assert len(segments) == 1


async def test_segments_whisper_rates_as_non_speech_are_dropped():
    # Arrange
    def fake_transcribe(_audio: np.ndarray) -> list[RecognizedSegment]:
        return [
            RecognizedSegment("ご視聴ありがとうございました", no_speech_prob=0.95),
            RecognizedSegment("こんにちは", no_speech_prob=0.05),
        ]

    transcripts = []
    backend = WhisperLocalBackend(transcribe=fake_transcribe, max_segment_sec=1.0, no_speech_threshold=0.6)
    await backend.start(transcripts.append)

    # Act
    await backend.send_pcm(tone(1.0))
    await backend.close()

    # Assert
    assert [transcript.text for transcript in transcripts] == ["こんにちは"]
