FROM ghcr.io/astral-sh/uv:0.12.21@sha256:a7aed3216253ee804de3e2d8afa5073baa1a177335345d43845cd4165e43b711 AS uv

# The moqt bindings (crates/python) build into an abi3 wheel for Python 3.11+, so the Debian
# python3 that runs maturin here need not be 3.11.
FROM rust:1.99.0-slim-trixie@sha256:2752b332db73fdbb7dc576f06c82ed1f312005784ef913d7e04a28f5f55dc581 AS moqt-wheel
COPY --from=uv /uv /usr/local/bin/uv
RUN apt-get update && apt-get install -y --no-install-recommends python3 && rm -rf /var/lib/apt/lists/*
WORKDIR /workspace
COPY . .
RUN UV_PYTHON_DOWNLOADS=never uv build --wheel --python python3 --out-dir /wheels crates/python

FROM python:3.11-slim-trixie@sha256:0dd364ba7e10242f07755449e3a3d0e35f9efd987952737b90def6709ab0c5ce
COPY --from=uv /uv /usr/local/bin/uv
ENV UV_COMPILE_BYTECODE=1 UV_LINK_MODE=copy UV_PYTHON_DOWNLOADS=never
RUN apt-get update && apt-get upgrade -y && rm -rf /var/lib/apt/lists/*
RUN pip uninstall -y setuptools

# The bots keep their repository depth so the lock's ../../../crates/python path resolves; the /app
# symlink keeps /app/<bot> as the path the services run.
WORKDIR /workspace/examples/python
RUN ln -s /workspace/examples/python /app
# Copy only what each bot needs: a developer's checkout keeps an untracked .env next to them.
# The bots install the room server from its source, so it comes before their dependencies.
COPY examples/python/moq-bot-rooms/pyproject.toml moq-bot-rooms/
COPY examples/python/moq-bot-rooms/moq_bot_rooms moq-bot-rooms/moq_bot_rooms
COPY examples/python/moq-chat-moderation/pyproject.toml examples/python/moq-chat-moderation/uv.lock moq-chat-moderation/
COPY examples/python/moq-camera-detection/pyproject.toml examples/python/moq-camera-detection/uv.lock moq-camera-detection/
COPY examples/python/moq-ptz-tracking/pyproject.toml examples/python/moq-ptz-tracking/uv.lock moq-ptz-tracking/
COPY --from=moqt-wheel /wheels /wheels
# This stage has no crates/python for the lock's path source, so uv skips moqt and the prebuilt
# wheel is installed instead.
RUN uv sync --frozen --no-dev --no-install-project --no-install-package moqt --directory moq-chat-moderation \
    && uv pip install --python moq-chat-moderation/.venv/bin/python --no-deps /wheels/moqt-*.whl \
    && uv sync --frozen --no-dev --no-install-project --directory moq-camera-detection \
    && uv sync --frozen --no-dev --no-install-project --directory moq-ptz-tracking
COPY examples/python/moq-chat-moderation/moq_chat_moderation moq-chat-moderation/moq_chat_moderation
COPY examples/python/moq-camera-detection/moq_camera_detection moq-camera-detection/moq_camera_detection
COPY examples/python/moq-ptz-tracking/moq_ptz_tracking moq-ptz-tracking/moq_ptz_tracking
