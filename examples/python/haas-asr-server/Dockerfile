FROM ghcr.io/astral-sh/uv:0.13.4@sha256:9f130421e930bfbdc563c00740466603f8723158e90297d07957c2294640262b AS uv

FROM python:3.11-slim-trixie@sha256:0dd364ba7e10242f07755449e3a3d0e35f9efd987952737b90def6709ab0c5ce
COPY --from=uv /uv /usr/local/bin/uv
ENV UV_LINK_MODE=copy UV_PYTHON_DOWNLOADS=never
# uv fetches haas-asr with git, and Triton compiles flash-linear-attention's kernels with a C compiler
# when the server first runs them.
RUN apt-get update && apt-get upgrade -y \
    && apt-get install -y --no-install-recommends git gcc libc6-dev \
    && rm -rf /var/lib/apt/lists/*
RUN pip uninstall -y setuptools

WORKDIR /app
COPY pyproject.toml uv.lock ./
RUN uv sync --frozen --no-dev --no-install-project --no-cache --extra model
COPY haas_asr_server haas_asr_server
RUN uv sync --frozen --no-dev --no-cache --extra model
# Bake the weights into the image so a cold start does not wait for a download.
ARG HAAS_REVISION=660728d39019e5e065046154beca44ac9a66a540
RUN .venv/bin/hf download third-intelligence/Haas-ja-v1.0 Haas-ja-v1.0.nemo --revision ${HAAS_REVISION} \
        --local-dir /opt/haas \
    && rm -rf /opt/haas/.cache
ENV HF_HUB_OFFLINE=1
EXPOSE 8080
CMD [".venv/bin/haas-asr-server", "--model", "/opt/haas/Haas-ja-v1.0.nemo", "--host", "0.0.0.0", "--port", "8080"]
