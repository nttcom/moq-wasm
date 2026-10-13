import av

from moq_ptz_tracking.video import JPEG_WIDTH, GroupDecoder, picture_to_jpeg
from tests.helpers import h264_group

JPEG_START_OF_IMAGE = b"\xff\xd8"


def test_every_frame_of_a_group_decodes_as_it_arrives():
    # Arrange
    decoder = GroupDecoder()

    # Act
    pictures = [decoder.decode(payload) for payload in h264_group(5)]

    # Assert
    assert all(picture is not None for picture in pictures)


def test_decoded_picture_becomes_a_jpeg_scaled_to_the_jpeg_width():
    # Arrange
    picture = GroupDecoder().decode(h264_group(1, width=1920, height=1080)[0])

    # Act
    jpeg = picture_to_jpeg(picture)

    # Assert
    assert jpeg.startswith(JPEG_START_OF_IMAGE)
    [decoded] = av.CodecContext.create("mjpeg", "r").decode(av.Packet(jpeg))
    assert (decoded.width, decoded.height) == (JPEG_WIDTH, 360)


def test_bytes_without_a_picture_give_nothing():
    # Act / Assert
    assert GroupDecoder().decode(b"\x00\x00\x00\x01") is None
