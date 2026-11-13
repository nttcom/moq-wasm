from dataclasses import dataclass, field
from typing import Any, Literal

from moq_wasm._native import parse_catalog

CATALOG_TRACK_NAME = "catalog"


@dataclass(frozen=True)
class CatalogTrack:
    """A track of an MSF catalog (draft-ietf-moq-msf-01 §5.2); `fields` holds every field as it
    was written, including those without an attribute here."""

    name: str
    namespace: list[str] | None = None
    role: str | None = None
    packaging: str | None = None
    codec: str | None = None
    bitrate: int | None = None
    width: int | None = None
    height: int | None = None
    framerate: float | None = None
    samplerate: int | None = None
    fields: dict[str, Any] = field(default_factory=dict, compare=False, repr=False)

    @classmethod
    def from_json(cls, track: dict[str, Any]) -> "CatalogTrack":
        namespace = track.get("namespace")
        return cls(
            name=track["name"],
            namespace=namespace.split("/") if namespace else None,
            role=track.get("role"),
            packaging=track.get("packaging"),
            codec=track.get("codec"),
            bitrate=track.get("bitrate"),
            width=track.get("width"),
            height=track.get("height"),
            framerate=track.get("framerate"),
            samplerate=track.get("samplerate"),
            fields=track,
        )


@dataclass(frozen=True)
class Catalog:
    """An independent draft-01 MSF catalog."""

    tracks: list[CatalogTrack]

    @classmethod
    def parse(cls, payload: bytes) -> "Catalog":
        """Raises ValueError for what `moq_wasm.parse_catalog` refuses."""
        return cls([CatalogTrack.from_json(track) for track in parse_catalog(payload).get("tracks", [])])

    def select(
        self,
        *,
        role: str | None = None,
        packaging: str | None = None,
        codec: str | None = None,
        prefer: Literal["lowest_bitrate", "highest_bitrate"] | None = None,
    ) -> CatalogTrack | None:
        """The track whose fields match those given, the first in the catalog or the one `prefer`
        names. `codec` matches the codec string or its first element: `"avc1"` matches
        `"avc1.42e01f"`. A track without a bitrate counts as the highest."""
        matching = [
            track
            for track in self.tracks
            if (role is None or track.role == role)
            and (packaging is None or track.packaging == packaging)
            and (codec is None or _codec_matches(track.codec, codec))
        ]
        if not matching:
            return None
        if prefer == "lowest_bitrate":
            return min(matching, key=_bitrate_or_highest)
        if prefer == "highest_bitrate":
            return max(matching, key=_bitrate_or_highest)
        return matching[0]


def _codec_matches(track_codec: str | None, codec: str) -> bool:
    return track_codec is not None and (track_codec == codec or track_codec.split(".")[0] == codec)


def _bitrate_or_highest(track: CatalogTrack) -> float:
    return float("inf") if track.bitrate is None else track.bitrate
