FROM pytorch/pytorch:2.13.0-cuda13.0-cudnn9-runtime
# The pytorch image's Python refuses pip installs into the system packages (PEP 668).
ENV PIP_BREAK_SYSTEM_PACKAGES=1
WORKDIR /app
COPY pyproject.toml ./
COPY system_one_server system_one_server
RUN pip install --no-cache-dir .
