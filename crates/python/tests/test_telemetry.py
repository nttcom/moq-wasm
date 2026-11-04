import moqt

from .test_session import connect


async def test_a_connection_is_taken_as_a_span_once_telemetry_started(relay_url):
    # Arrange
    moqt.start_telemetry()
    moqt.take_telemetry()

    # Act
    session = await connect(relay_url)
    await session.close()
    records = moqt.take_telemetry()

    # Assert
    connects = [span for span in records["spans"] if span["name"] == "moqt.session.connect"]
    assert [span["fields"]["url"] for span in connects] == [relay_url]
    assert len(connects[0]["traceId"]) == 32
    assert moqt.take_telemetry() == {"logs": [], "spans": []}
