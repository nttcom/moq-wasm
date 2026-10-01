from moq_camera_detection.keyframe import keyframe_to_jpeg
from tests.helpers import h264_keyframe

JPEG_START_OF_IMAGE = b"\xff\xd8"


def test_annex_b_keyframe_becomes_a_jpeg():
    # Act
    jpeg = keyframe_to_jpeg(h264_keyframe())

    # Assert
    assert jpeg is not None and jpeg.startswith(JPEG_START_OF_IMAGE)


def test_bytes_without_a_picture_give_nothing():
    # Act / Assert
    assert keyframe_to_jpeg(b"\x00\x00\x00\x01") is None
