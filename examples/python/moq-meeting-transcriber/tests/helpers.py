import asyncio
import json

import av


def opus_packets(seconds: float, frequency: float = 440.0) -> list[bytes]:
    """20 ms Opus packets of a 48 kHz mono tone, as a Meeting participant's browser sends them."""
    encoder = av.CodecContext.create("libopus", "w")
    encoder.sample_rate = 48_000
    encoder.layout = "mono"
    encoder.format = "s16"
    frames = []
    for index in range(int(seconds * 50)):
        frame = av.AudioFrame.from_ndarray(_tone(index, frequency), format="s16", layout="mono")
        frame.sample_rate = 48_000
        frame.pts = index * 960
        frames.append(frame)
    return [bytes(packet) for frame in [*frames, None] for packet in encoder.encode(frame)]


def _tone(index: int, frequency: float):
    import numpy as np

    t = (np.arange(960) + index * 960) / 48_000
    return (np.sin(2 * np.pi * frequency * t) * 8000).astype(np.int16).reshape(1, -1)


def meeting_catalog(*audio_bitrates: int) -> bytes:
    tracks = [
        {"name": f"audio_{bitrate // 1000}kbps", "role": "audio", "packaging": "loc", "codec": "opus", "bitrate": bitrate}
        for bitrate in audio_bitrates
    ]
    tracks.append({"name": "camera_720p", "role": "video", "packaging": "loc", "codec": "avc1.64001f", "bitrate": 2_500_000})
    return json.dumps({"version": 1, "tracks": tracks}).encode()


class RecordingPublication:
    """Keeps what the timeline writes, as (new_group, records) pairs."""

    def __init__(self):
        self.writes: list[tuple[bool, list[dict]]] = []

    async def write(self, payload: bytes, *, new_group: bool = False) -> None:
        self.writes.append((new_group, json.loads(payload)))


class FakeClock:
    def __init__(self, now: float):
        self.now = now

    def __call__(self) -> float:
        return self.now


class SessionRefusingNamespaces:
    """A relay session that stays open but fails SUBSCRIBE_NAMESPACE."""

    async def track(self, namespace: list[str], name: str, *, retention_seconds: float) -> RecordingPublication:
        return RecordingPublication()

    async def publish_namespace(self, namespace: list[str]) -> None:
        pass

    async def subscribe_namespace(self, prefix: list[str]):
        raise RuntimeError("SUBSCRIBE_NAMESPACE refused")

    def closed(self) -> asyncio.Future:
        return asyncio.get_running_loop().create_future()
