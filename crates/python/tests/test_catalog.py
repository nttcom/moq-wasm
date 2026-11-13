import json

import moq_wasm
import pytest


def test_parse_catalog_reads_a_draft_01_catalog():
    # Arrange
    document = {"version": "draft-01", "tracks": [{"name": "video", "packaging": "loc", "isLive": True}]}

    # Act / Assert
    assert moq_wasm.parse_catalog(json.dumps(document).encode()) == document


@pytest.mark.parametrize(
    "payload",
    [
        b'{"version":"draft-03","tracks":[]}',
        b'{"deltaUpdate":[{"op":"remove","tracks":[{"name":"video"}]}]}',
        b"not json",
    ],
)
def test_parse_catalog_refuses_another_version_a_delta_update_and_what_is_not_a_catalog(payload):
    # Act / Assert
    with pytest.raises(ValueError):
        moq_wasm.parse_catalog(payload)
