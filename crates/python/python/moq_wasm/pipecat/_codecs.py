"""The PyAV codecs of `MoqWasmTransport`: the SDK's wheel carries none (docs/architecture/python/sdk.md
"No codec in the wheel")."""

from fractions import Fraction

import av
import numpy as np

OPUS_SAMPLE_RATE = 48_000
OPUS_BITRATE = 32_000

# A video codec string's first element, as the catalog writes it, to the PyAV decoder.
VIDEO_DECODERS = {"avc1": "h264", "avc3": "h264", "hvc1": "hevc", "hev1": "hevc", "vp8": "vp8", "vp09": "vp9", "av01": "av1"}


class OpusDecoder:
    """Turns the Opus packets of a LOC audio track into mono 16-bit PCM at `sample_rate`."""

    def __init__(self, sample_rate: int):
        self._decoder = av.CodecContext.create("opus", "r")
        self._resampler = av.AudioResampler(format="s16", layout="mono", rate=sample_rate)

    def decode(self, packet: bytes) -> bytes:
        return b"".join(
            bytes(resampled.planes[0])[: resampled.samples * 2]
            for decoded in self._decoder.decode(av.Packet(packet))
            for resampled in self._resampler.resample(decoded)
        )


class OpusEncoder:
    """Turns mono 16-bit PCM at any rate into 20 ms Opus packets at 48 kHz."""

    def __init__(self):
        self._encoder = av.CodecContext.create("libopus", "w")
        self._encoder.sample_rate = OPUS_SAMPLE_RATE
        self._encoder.layout = "mono"
        self._encoder.format = "s16"
        self._encoder.bit_rate = OPUS_BITRATE
        self._encoder.open()
        self._resampler = av.AudioResampler(format="s16", layout="mono", rate=OPUS_SAMPLE_RATE)
        self._fifo = av.AudioFifo()
        self.frame_samples = self._encoder.frame_size

    def encode(self, pcm: bytes, sample_rate: int) -> list[bytes]:
        frame = av.AudioFrame.from_ndarray(np.frombuffer(pcm, np.int16).reshape(1, -1), format="s16", layout="mono")
        frame.sample_rate = sample_rate
        for resampled in self._resampler.resample(frame):
            resampled.pts = None
            self._fifo.write(resampled)
        packets = []
        while (chunk := self._fifo.read(self.frame_samples)) is not None:
            chunk.pts = None
            packets.extend(bytes(packet) for packet in self._encoder.encode(chunk))
        return packets


class VideoDecoder:
    """Decodes one group of a LOC video track into pictures. A group starts at a keyframe, so
    each group gets a decoder of its own; a group joined in the middle decodes to nothing."""

    def __init__(self, codec: str):
        name = VIDEO_DECODERS.get(codec.split(".")[0])
        if name is None:
            raise ValueError(f"no decoder for video codec {codec}")
        self._decoder = av.CodecContext.create(name, "r")

    def decode(self, payload: bytes) -> av.VideoFrame | None:
        try:
            pictures = self._decoder.decode(av.Packet(payload))
        except av.FFmpegError:
            return None
        return pictures[-1] if pictures else None


class VideoEncoder:
    """Encodes RGB pictures into H.264 (Annex B, its parameter sets in every keyframe) or VP8. A
    keyframe comes every two seconds, and on `request_keyframe`."""

    def __init__(self, codec: str, width: int, height: int, framerate: int):
        if codec == "H264":
            self.codec_string = "avc1.42e01f"
            self._encoder = av.CodecContext.create("libx264", "w")
            self._encoder.options = {
                "preset": "ultrafast",
                "tune": "zerolatency",
                "profile": "baseline",
                "x264-params": f"repeat-headers=1:keyint={2 * framerate}:scenecut=0",
            }
        elif codec == "VP8":
            self.codec_string = "vp8"
            self._encoder = av.CodecContext.create("libvpx", "w")
            self._encoder.options = {"deadline": "realtime", "lag-in-frames": "0"}
            self._encoder.gop_size = 2 * framerate
        else:
            raise ValueError(f"unsupported video_out_codec {codec!r}: use 'H264' or 'VP8'")
        self._encoder.width = width
        self._encoder.height = height
        self._encoder.pix_fmt = "yuv420p"
        self._encoder.time_base = Fraction(1, framerate)
        self._encoder.framerate = framerate
        self._encoder.open()
        self._width = width
        self._height = height
        self._frames = 0
        self._keyframe_requested = False

    def request_keyframe(self) -> None:
        self._keyframe_requested = True

    def encode(self, image: bytes, size: tuple[int, int], color_format: str) -> list[bytes]:
        """`image` is `size` pixels in `color_format` (`RGB` or `RGBA`), scaled to the encoder's."""
        channels = 4 if color_format == "RGBA" else 3
        picture = av.VideoFrame.from_ndarray(
            np.frombuffer(image, np.uint8).reshape(size[1], size[0], channels),
            format="rgba" if channels == 4 else "rgb24",
        ).reformat(width=self._width, height=self._height, format="yuv420p")
        picture.pts = self._frames
        self._frames += 1
        if self._keyframe_requested:
            picture.pict_type = av.video.frame.PictureType.I
            self._keyframe_requested = False
        return [bytes(packet) for packet in self._encoder.encode(picture)]
