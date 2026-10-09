FROM ghcr.io/astral-sh/uv:0.12.21@sha256:a7aed3216253ee804de3e2d8afa5073baa1a177335345d43845cd4165e43b711 AS uv

FROM python:3.11-slim-trixie@sha256:0dd364ba7e10242f07755449e3a3d0e35f9efd987952737b90def6709ab0c5ce
COPY --from=uv /uv /usr/local/bin/uv
ENV UV_COMPILE_BYTECODE=1 UV_LINK_MODE=copy UV_PYTHON_DOWNLOADS=never
RUN apt-get update && apt-get upgrade -y && rm -rf /var/lib/apt/lists/*

WORKDIR /app
# Copy only what each bot needs: a developer's checkout keeps an untracked .env next to them.
COPY examples/python/moq-chat-moderation/pyproject.toml examples/python/moq-chat-moderation/uv.lock moq-chat-moderation/
COPY examples/python/moq-camera-detection/pyproject.toml examples/python/moq-camera-detection/uv.lock moq-camera-detection/
RUN uv sync --frozen --no-dev --no-install-project --directory moq-chat-moderation \
    && uv sync --frozen --no-dev --no-install-project --directory moq-camera-detection
COPY examples/python/moq-chat-moderation/moq_chat_moderation moq-chat-moderation/moq_chat_moderation
COPY examples/python/moq-camera-detection/moq_camera_detection moq-camera-detection/moq_camera_detection
