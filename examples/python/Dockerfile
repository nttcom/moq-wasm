FROM ghcr.io/astral-sh/uv:0.13.4@sha256:9f130421e930bfbdc563c00740466603f8723158e90297d07957c2294640262b AS uv

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
# The bots install moq-bot-rooms and moq-d3-flash-bot from source, so both are copied before uv sync.
COPY examples/python/moq-bot-rooms/pyproject.toml moq-bot-rooms/
COPY examples/python/moq-bot-rooms/moq_bot_rooms moq-bot-rooms/moq_bot_rooms
COPY examples/python/moq-d3-flash-bot/pyproject.toml moq-d3-flash-bot/
COPY examples/python/moq-d3-flash-bot/moq_d3_flash_bot moq-d3-flash-bot/moq_d3_flash_bot
COPY examples/python/moq-chat-moderation/pyproject.toml examples/python/moq-chat-moderation/uv.lock moq-chat-moderation/
COPY examples/python/moq-camera-detection/pyproject.toml examples/python/moq-camera-detection/uv.lock moq-camera-detection/
COPY examples/python/moq-ptz-tracking/pyproject.toml examples/python/moq-ptz-tracking/uv.lock moq-ptz-tracking/
COPY examples/python/moq-meeting-transcriber/pyproject.toml examples/python/moq-meeting-transcriber/uv.lock moq-meeting-transcriber/
COPY --from=moqt-wheel /wheels /wheels
# This stage has no crates/python for the lock's path source, so uv skips moqt and the prebuilt
# wheel is installed instead.
RUN for bot in moq-chat-moderation moq-camera-detection moq-ptz-tracking moq-meeting-transcriber; do \
        uv sync --frozen --no-dev --no-install-project --no-install-package moqt --directory $bot \
        && uv pip install --python $bot/.venv/bin/python --no-deps /wheels/moqt-*.whl || exit 1; \
    done
COPY examples/python/moq-chat-moderation/moq_chat_moderation moq-chat-moderation/moq_chat_moderation
COPY examples/python/moq-camera-detection/moq_camera_detection moq-camera-detection/moq_camera_detection
COPY examples/python/moq-ptz-tracking/moq_ptz_tracking moq-ptz-tracking/moq_ptz_tracking
COPY examples/python/moq-meeting-transcriber/moq_meeting_transcriber moq-meeting-transcriber/moq_meeting_transcriber
# Bake the transcriber's CPU models, its default Whisper and Granite Speech, into the image so a new
# room does not wait for a download, and Cloud Run does not hold the files in the instance's memory.
ENV HF_HOME=/opt/huggingface
RUN cd moq-meeting-transcriber && .venv/bin/python -c \
    "from faster_whisper import download_model; from moq_meeting_transcriber.bot import DEFAULT_MODEL; download_model(DEFAULT_MODEL)" \
    && .venv/bin/python -c \
    "from huggingface_hub import snapshot_download; from moq_meeting_transcriber.granite import MODEL; snapshot_download(MODEL)"
