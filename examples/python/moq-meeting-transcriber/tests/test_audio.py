import numpy as np

from moq_meeting_transcriber.audio import SAMPLE_RATE, OpusDecoder
from tests.helpers import opus_packets


def test_decodes_opus_packets_into_16_khz_mono_pcm():
    # Arrange
    decoder = OpusDecoder()
    packets = opus_packets(seconds=1.0)

    # Act
    pcm = b"".join(decoder.decode(packet) for packet in packets)

    # Assert: about one second of samples, carrying the tone rather than silence
    samples = np.frombuffer(pcm, dtype=np.int16)
    assert abs(samples.size - SAMPLE_RATE) < SAMPLE_RATE * 0.05
    assert np.abs(samples).max() > 1000
