from moq_meeting_transcriber.catalog import audio_track_name
from tests.helpers import meeting_catalog


def test_picks_the_audio_track_with_the_highest_bitrate():
    # Act / Assert
    assert audio_track_name(meeting_catalog(32_000, 128_000, 64_000)) == "audio_128kbps"


def test_a_catalog_without_audio_has_no_track():
    # Act / Assert
    assert audio_track_name(meeting_catalog()) is None
