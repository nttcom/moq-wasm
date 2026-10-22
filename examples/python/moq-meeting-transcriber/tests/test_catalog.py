from moq_meeting_transcriber.catalog import audio_track_name
from tests.helpers import meeting_catalog


def test_picks_the_audio_track_with_the_lowest_bitrate():
    # Act / Assert
    assert audio_track_name(meeting_catalog(128_000, 32_000, 64_000)) == "audio_32kbps"


def test_a_catalog_without_audio_has_no_track():
    # Act / Assert
    assert audio_track_name(meeting_catalog()) is None
