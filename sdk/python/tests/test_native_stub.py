import ast
import asyncio
from pathlib import Path

import pytest

from moq_wasm import _native

STUB = Path(_native.__file__).with_name("_native.pyi")


def stub_members() -> list[tuple[str, str | None]]:
    """Each module-level function and class of the stub as (name, None), and each method and
    property of a class as (class, member); attributes, such as those `RequestError` gets when it
    is raised, are left out."""
    members: list[tuple[str, str | None]] = []
    for node in ast.parse(STUB.read_text()).body:
        if isinstance(node, ast.FunctionDef):
            members.append((node.name, None))
        elif isinstance(node, ast.ClassDef):
            members.append((node.name, None))
            for item in node.body:
                if isinstance(item, ast.FunctionDef):
                    members.append((node.name, item.name))
    return members


@pytest.mark.parametrize(("owner", "member"), stub_members())
def test_every_name_the_stub_declares_exists_in_the_native_module(owner, member):
    # Act / Assert
    native = getattr(_native, owner)
    if member is not None and member != "__init__":
        assert hasattr(native, member)


async def test_a_native_request_returns_a_future():
    # Act: a connect offering no version fails at once
    connecting = _native.connect("moqt://127.0.0.1:9", verify_certificate=False, versions=[])

    # Assert
    assert isinstance(connecting, asyncio.Future)
    with pytest.raises(RuntimeError):
        await connecting


def test_a_location_cannot_be_changed():
    # Arrange
    location = _native.Location(1, 2)

    # Act / Assert
    with pytest.raises(AttributeError):
        location.group_id = 3  # type: ignore[misc]
