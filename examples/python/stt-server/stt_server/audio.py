"""Decodes MoQT audio objects to PCM16 for the pipeline and encodes reply PCM to Opus."""

import base64
import json
import struct
from dataclasses import dataclass

import av

from .pipeline.base import PIPELINE_SAMPLE_RATE

CATALOG_TRACK_NAME = "catalog"


@dataclass(frozen=True)
class AudioCodec:
    name: str
    sample_rate: int
    channels: int
    description: bytes | None = None

    def ffmpeg_decoder(self) -> str:
        if self.name == "opus":
            return "opus"
        if self.name.startswith("mp4a.40"):
            return "aac"
        raise ValueError(f"unsupported audio codec: {self.name}")


@dataclass(frozen=True)
class AudioTrack:
    namespace: str
    name: str
    codec: AudioCodec


def channels_from_config(channel_config: str | None) -> int:
    if channel_config in (None, "", "mono", "1"):
        return 1
    return 2


def audio_tracks_from_catalog(namespace: str, catalog_json: bytes) -> list[AudioTrack]:
    """MSF catalog (draft-ietf-moq-msf-00 §5): audio tracks are those with
    role "audio" or an audio codec; `samplerate` / `channelConfig` describe
    the encoded stream and `initData` carries codec description if any."""
    catalog = json.loads(catalog_json)
    tracks = (catalog.get("tracks") or []) + (catalog.get("addTracks") or [])
    audio_tracks = []
    for track in tracks:
        codec_name = track.get("codec")
        is_audio = track.get("role") == "audio" or (
            codec_name is not None and (codec_name == "opus" or codec_name.startswith("mp4a"))
        )
        if not is_audio or codec_name is None:
            continue
        init_data = track.get("initData")
        audio_tracks.append(
            AudioTrack(
                namespace=track.get("namespace") or namespace,
                name=track["name"],
                codec=AudioCodec(
                    name=codec_name,
                    sample_rate=int(track.get("samplerate") or 48000),
                    channels=channels_from_config(track.get("channelConfig")),
                    description=base64.b64decode(init_data) if init_data else None,
                ),
            )
        )
    return audio_tracks


@dataclass(frozen=True)
class AudioPacket:
    data: bytes
    codec: AudioCodec | None


def parse_audio_object(payload: bytes) -> AudioPacket:
    """Two payload layouts exist in this repository: the browser sends the
    bare EncodedAudioChunk bytes, while bridges/live-ingest prefixes the
    frame with a 4-byte big-endian length and a JSON metadata object that
    also names the codec."""
    if len(payload) >= 5:
        (meta_len,) = struct.unpack(">I", payload[:4])
        if 4 + meta_len <= len(payload) and payload[4:5] == b"{":
            meta = json.loads(payload[4 : 4 + meta_len])
            description = meta.get("descriptionBase64")
            codec = AudioCodec(
                name=meta.get("codec", "opus"),
                sample_rate=int(meta.get("sampleRate") or 48000),
                channels=int(meta.get("channels") or 1),
                description=base64.b64decode(description) if description else None,
            )
            return AudioPacket(data=payload[4 + meta_len :], codec=codec)
    return AudioPacket(data=payload, codec=None)


class AudioDecoder:
    """Decodes one codec stream and resamples every frame to mono PCM16 at
    `target_sample_rate`."""

    def __init__(self, codec: AudioCodec, target_sample_rate: int = PIPELINE_SAMPLE_RATE) -> None:
        self.context = av.CodecContext.create(codec.ffmpeg_decoder(), "r")
        self.context.sample_rate = codec.sample_rate
        self.context.layout = "mono" if codec.channels == 1 else "stereo"
        if codec.description:
            self.context.extradata = codec.description
        self.resampler = av.AudioResampler(format="s16", layout="mono", rate=target_sample_rate)

    def decode(self, packet: bytes) -> bytes:
        pcm = bytearray()
        for frame in self.context.decode(av.Packet(packet)):
            for resampled in self.resampler.resample(frame):
                pcm += bytes(resampled.planes[0])[: resampled.samples * 2]
        return bytes(pcm)


OPUS_SAMPLE_RATE = 48000
OPUS_BITRATE = 64_000


def encode_opus(pcm: bytes, sample_rate: int) -> list[bytes]:
    """Encodes one mono PCM buffer into 20 ms Opus packets at 48 kHz."""
    context = av.CodecContext.create("libopus", "w")
    context.sample_rate = OPUS_SAMPLE_RATE
    context.layout = "mono"
    context.format = "s16"
    context.bit_rate = OPUS_BITRATE
    context.open()
    resampler = av.AudioResampler(format="s16", layout="mono", rate=OPUS_SAMPLE_RATE)
    frame = av.AudioFrame(format="s16", layout="mono", samples=len(pcm) // 2)
    frame.sample_rate = sample_rate
    frame.planes[0].update(pcm)
    packets = []
    pts = 0
    for resampled in resampler.resample(frame):
        resampled.pts = pts
        pts += resampled.samples
        packets += [bytes(packet) for packet in context.encode(resampled)]
    return packets + [bytes(packet) for packet in context.encode(None)]
