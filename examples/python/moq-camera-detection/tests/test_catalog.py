import json

from moq_wasm.media import Catalog

from moq_camera_detection.catalog import camera_video_track


# The fields draft-ietf-moq-msf-02 §5.2.4.1 requires of a LOC track of each role.
REQUIRED_FIELDS = {
    "video": {"codec": "avc1.42e01f", "bitrate": 1_000_000, "framerate": 30},
    "audio": {"codec": "opus", "bitrate": 64_000, "samplerate": 48_000, "channelConfig": "mono"},
}


def catalog(*tracks: dict) -> Catalog:
    listed = [
        {"isLive": True, **(REQUIRED_FIELDS.get(track["role"], {}) if track["packaging"] == "loc" else {}), **track}
        for track in tracks
    ]
    return Catalog.parse(json.dumps({"version": "draft-02", "tracks": listed}).encode())


def test_picks_the_video_a_rendition_is_named_after():
    # Arrange
    tracks = catalog(
        {"name": "camera_low", "role": "video", "packaging": "loc"},
        {"name": "camera", "role": "video", "packaging": "loc"},
        {"name": "camera_cmaf", "role": "video", "packaging": "cmaf"},
        {"name": "camera_timeline", "role": "mediatimeline", "packaging": "mediatimeline"},
    )

    # Act / Assert
    assert camera_video_track(tracks).name == "camera"


def test_a_catalog_without_video_has_no_track():
    # Act / Assert
    assert camera_video_track(catalog({"name": "camera_audio", "role": "audio", "packaging": "loc"})) is None
