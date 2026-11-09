import pytest
from moq_system_one_bot.testing import fake_system_one


@pytest.fixture
async def fake_system_one_server():
    async with fake_system_one({"l0": {"type": "noul", "noul": 0.9}, "l1": {"type": "noul", "noul": 0.1}}) as server:
        yield server
