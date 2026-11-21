import av
from moq_wasm.codecs import VideoDecoder

from moq_system_one_bot.video import picture_to_jpeg
from tests.helpers import h264_group

JPEG_START_OF_IMAGE = b"\xff\xd8"


def jpeg_size(jpeg: bytes) -> tuple[int, int]:
    [decoded] = av.CodecContext.create("mjpeg", "r").decode(av.Packet(jpeg))
    return decoded.width, decoded.height


def test_decoded_picture_becomes_a_jpeg_of_its_own_size():
    # Arrange
    [(picture, _)] = VideoDecoder("avc1").decode(h264_group(1)[0])

    # Act
    jpeg = picture_to_jpeg(picture)

    # Assert
    assert jpeg.startswith(JPEG_START_OF_IMAGE)
    assert jpeg_size(jpeg) == (64, 48)


def test_decoded_picture_becomes_a_jpeg_scaled_to_the_given_width():
    # Arrange
    [(picture, _)] = VideoDecoder("avc1").decode(h264_group(1, width=1920, height=1080)[0])

    # Act
    jpeg = picture_to_jpeg(picture, width=640)

    # Assert
    assert jpeg.startswith(JPEG_START_OF_IMAGE)
    assert jpeg_size(jpeg) == (640, 360)
