import pytest
from moq_system_one_bot.testing import fake_system_one


@pytest.fixture
async def fake_vision_server():
    async with fake_system_one({"t0": {"boxes": []}, "t1": {"boxes": []}}) as server:
        yield server
