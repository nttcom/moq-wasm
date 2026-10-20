import av

SAMPLE_RATE = 16_000


class OpusDecoder:
    """Turns the Opus packets of a LOC audio track into 16 kHz mono 16-bit PCM, the input of the speech recognizer."""

    def __init__(self):
        self._decoder = av.CodecContext.create("opus", "r")
        self._resampler = av.AudioResampler(format="s16", layout="mono", rate=SAMPLE_RATE)

    def decode(self, packet: bytes) -> bytes:
        return b"".join(
            bytes(resampled.planes[0])[: resampled.samples * 2]
            for decoded in self._decoder.decode(av.Packet(packet))
            for resampled in self._resampler.resample(decoded)
        )
