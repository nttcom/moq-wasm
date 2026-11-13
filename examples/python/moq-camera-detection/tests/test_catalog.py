import json

from moq_camera_detection.catalog import camera_video_track


def catalog(*tracks: dict, version: str = "draft-01") -> bytes:
    return json.dumps({"version": version, "tracks": [{"isLive": True, **track} for track in tracks]}).encode()


def test_picks_the_video_a_rendition_is_named_after():
    # Arrange
    tracks = catalog(
        {"name": "camera_low", "role": "video", "packaging": "loc"},
        {"name": "camera", "role": "video", "packaging": "loc"},
        {"name": "camera_cmaf", "role": "video", "packaging": "cmaf"},
        {"name": "camera_timeline", "role": "mediatimeline", "packaging": "mediatimeline"},
    )

    # Act / Assert
    assert camera_video_track(tracks) == "camera"


def test_a_catalog_without_video_has_no_track():
    # Act / Assert
    assert camera_video_track(catalog({"name": "camera_audio", "role": "audio", "packaging": "loc"})) is None


def test_a_catalog_of_another_version_has_no_track():
    # Act / Assert
    assert camera_video_track(catalog({"name": "camera", "role": "video", "packaging": "loc"}, version="draft-03")) is None
