FROM ghcr.io/astral-sh/uv:0.13.4@sha256:9f130421e930bfbdc563c00740466603f8723158e90297d07957c2294640262b AS uv

# The moq-wasm SDK (crates/python) builds into an abi3 wheel for Python 3.11+, so the Debian
# python3 that runs maturin here need not be 3.11.
FROM rust:1.100.0-slim-trixie@sha256:6f14b2b4b278ec0d50aa1d69541c304ce0f95e0f6268487aa9924af1c2e9aebe AS moq-wasm-wheel
COPY --from=uv /uv /usr/local/bin/uv
RUN apt-get update && apt-get install -y --no-install-recommends python3 && rm -rf /var/lib/apt/lists/*
WORKDIR /workspace
COPY . .
RUN UV_PYTHON_DOWNLOADS=never uv build --wheel --python python3 --out-dir /wheels crates/python

# `bots`: every bot without the transcriber's models.
FROM python:3.11-slim-trixie@sha256:0dd364ba7e10242f07755449e3a3d0e35f9efd987952737b90def6709ab0c5ce AS bots
COPY --from=uv /uv /usr/local/bin/uv
ENV UV_COMPILE_BYTECODE=1 UV_LINK_MODE=copy UV_PYTHON_DOWNLOADS=never
RUN apt-get update && apt-get upgrade -y && rm -rf /var/lib/apt/lists/*
RUN pip uninstall -y setuptools

# The bots keep their repository depth so the lock's ../../../crates/python path resolves; the /app
# symlink keeps /app/<bot> as the path the services run.
WORKDIR /workspace/examples/python
RUN ln -s /workspace/examples/python /app
# Copy only what each bot needs: a developer's checkout keeps an untracked .env next to them.
# The bots install moq-bot-job and moq-system-one-bot from source, so both are copied before uv sync.
COPY examples/python/moq-bot-job/pyproject.toml moq-bot-job/
COPY examples/python/moq-bot-job/moq_bot_job moq-bot-job/moq_bot_job
COPY examples/python/moq-system-one-bot/pyproject.toml moq-system-one-bot/
COPY examples/python/moq-system-one-bot/moq_system_one_bot moq-system-one-bot/moq_system_one_bot
COPY examples/python/moq-chat-moderation/pyproject.toml examples/python/moq-chat-moderation/uv.lock moq-chat-moderation/
COPY examples/python/moq-camera-detection/pyproject.toml examples/python/moq-camera-detection/uv.lock moq-camera-detection/
COPY examples/python/moq-ptz-tracking/pyproject.toml examples/python/moq-ptz-tracking/uv.lock moq-ptz-tracking/
COPY examples/python/moq-meeting-transcriber/pyproject.toml examples/python/moq-meeting-transcriber/uv.lock moq-meeting-transcriber/
COPY examples/python/moq-ai-voice-agent/pyproject.toml examples/python/moq-ai-voice-agent/uv.lock moq-ai-voice-agent/
COPY --from=moq-wasm-wheel /wheels /wheels
# This stage has no crates/python for the lock's path source, so uv skips moq-wasm and the prebuilt
# wheel is installed instead.
RUN for bot in moq-chat-moderation moq-camera-detection moq-ptz-tracking moq-meeting-transcriber moq-ai-voice-agent; do \
        uv sync --frozen --no-dev --no-install-project --no-install-package moq-wasm --directory $bot \
        && uv pip install --python $bot/.venv/bin/python --no-deps /wheels/moq_wasm-*.whl || exit 1; \
    done
COPY examples/python/moq-chat-moderation/moq_chat_moderation moq-chat-moderation/moq_chat_moderation
COPY examples/python/moq-camera-detection/moq_camera_detection moq-camera-detection/moq_camera_detection
COPY examples/python/moq-ptz-tracking/moq_ptz_tracking moq-ptz-tracking/moq_ptz_tracking
COPY examples/python/moq-meeting-transcriber/moq_meeting_transcriber moq-meeting-transcriber/moq_meeting_transcriber
COPY examples/python/moq-ai-voice-agent/moq_ai_voice_agent moq-ai-voice-agent/moq_ai_voice_agent
# The bot service starts each bot as `python -m <package>.bot` from any directory.
RUN for bot in moq-chat-moderation moq-camera-detection moq-ptz-tracking moq-meeting-transcriber moq-ai-voice-agent; do \
        uv pip install --python $bot/.venv/bin/python --no-deps -e $bot || exit 1; \
    done

FROM rust:1.100.0-slim-trixie@sha256:6f14b2b4b278ec0d50aa1d69541c304ce0f95e0f6268487aa9924af1c2e9aebe AS bot-dispatcher
WORKDIR /workspace
COPY . .
RUN cargo build --locked --release -p bot-dispatcher

# `demo`: the bots with the bot service, which launches them as local processes (compose.demo.yml).
FROM bots AS demo
COPY --from=bot-dispatcher /workspace/target/release/bot-dispatcher /usr/local/bin/
CMD ["/usr/local/bin/bot-dispatcher"]

FROM bots

# Bake the transcriber's CPU models (the default Whisper and Granite Speech) into the image so a new
# room does not wait for a download and Cloud Run does not hold the files in the instance's memory.
ENV HF_HOME=/opt/huggingface
RUN cd moq-meeting-transcriber && .venv/bin/python -c \
    "from faster_whisper import download_model; from moq_meeting_transcriber.bot import DEFAULT_MODEL; download_model(DEFAULT_MODEL)" \
    && .venv/bin/python -c \
    "from huggingface_hub import snapshot_download; from moq_meeting_transcriber.granite import MODEL; snapshot_download(MODEL)"
