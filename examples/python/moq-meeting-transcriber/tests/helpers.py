import asyncio
import json
from types import SimpleNamespace

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


class NamedRecognizer:
    def __init__(self, name: str):
        self.name = name

    async def ready(self) -> None:
        pass


class SessionRefusingNamespaces:
    """A relay session that stays open but fails SUBSCRIBE_NAMESPACE."""

    def __init__(self):
        self.publications: dict[str, RecordingPublication] = {}

    async def track(self, namespace: list[str], name: str, *, retention_seconds: float) -> RecordingPublication:
        return self.publications.setdefault(name, RecordingPublication())

    async def publish_namespace(self, namespace: list[str]) -> None:
        pass

    async def subscribe_namespace(self, prefix: list[str]):
        raise RuntimeError("SUBSCRIBE_NAMESPACE refused")

    def closed(self) -> asyncio.Future:
        return asyncio.get_running_loop().create_future()


class SessionWithParticipants(SessionRefusingNamespaces):
    """A relay session whose namespace subscription yields what `publish` announces; subscribing to a
    participant's track fails."""

    def __init__(self):
        super().__init__()
        self._events: asyncio.Queue[SimpleNamespace] = asyncio.Queue()

    def publish(self, namespace: list[str], *, active: bool = True) -> None:
        self._events.put_nowait(SimpleNamespace(namespace=namespace, active=active))

    async def subscribe_namespace(self, prefix: list[str]):
        return self._iterate()

    async def _iterate(self):
        while True:
            yield await self._events.get()

    async def subscribe(self, namespace: list[str], name: str, *, current_group: bool = False):
        raise RuntimeError("no such track")
