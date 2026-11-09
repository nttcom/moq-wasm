import json

from moq_camera_detection.catalog import camera_video_track


def catalog(*tracks: dict) -> bytes:
    return json.dumps({"version": "draft-01", "tracks": list(tracks)}).encode()


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
