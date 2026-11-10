import av

from moq_system_one_bot.video import GroupDecoder, picture_to_jpeg
from tests.helpers import h264_group

JPEG_START_OF_IMAGE = b"\xff\xd8"


def jpeg_size(jpeg: bytes) -> tuple[int, int]:
    [decoded] = av.CodecContext.create("mjpeg", "r").decode(av.Packet(jpeg))
    return decoded.width, decoded.height


def test_every_frame_of_a_group_decodes_as_it_arrives():
    # Arrange
    decoder = GroupDecoder()

    # Act
    pictures = [decoder.decode(payload) for payload in h264_group(5)]

    # Assert
    assert all(picture is not None for picture in pictures)


def test_decoded_picture_becomes_a_jpeg_of_its_own_size():
    # Arrange
    picture = GroupDecoder().decode(h264_group(1)[0])

    # Act
    jpeg = picture_to_jpeg(picture)

    # Assert
    assert jpeg.startswith(JPEG_START_OF_IMAGE)
    assert jpeg_size(jpeg) == (64, 48)


def test_decoded_picture_becomes_a_jpeg_scaled_to_the_given_width():
    # Arrange
    picture = GroupDecoder().decode(h264_group(1, width=1920, height=1080)[0])

    # Act
    jpeg = picture_to_jpeg(picture, width=640)

    # Assert
    assert jpeg.startswith(JPEG_START_OF_IMAGE)
    assert jpeg_size(jpeg) == (640, 360)


def test_bytes_without_a_picture_give_nothing():
    # Act / Assert
    assert GroupDecoder().decode(b"\x00\x00\x00\x01") is None
