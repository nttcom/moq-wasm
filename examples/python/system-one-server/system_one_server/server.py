"""Serves a System One decision model over HTTP for the MoQ bots.

- `GET /health`: 200 once the model is loaded.
- `POST /v1/systemone`: `{"state", "images": [data URL, ...], "questions": {name: question}}`; the reply is the
  model's `{"answers", "usage"}`. A question the model rejects gets 422.
"""

import argparse
import asyncio
import base64
import binascii
import io
import logging
from typing import Any

from aiohttp import web
from PIL import Image, UnidentifiedImageError

logger = logging.getLogger(__name__)

DEFAULT_MODEL = "LiquidAI/d1-2B"
# The model runs code from its repository (trust_remote_code), so a Hub id is pinned to a reviewed commit.
DEFAULT_REVISION = "cfe5760ff5d5998f91d9ac56bd4b2dbd96996d41"
SYSTEM_ONE_PATH = "/v1/systemone"


def create_app(model: Any) -> web.Application:
    """`model` answers like d1-2B's `system_one(state, questions, media)`."""
    # One forward pass at a time: a d1-2B call takes tens of milliseconds on a GPU.
    lock = asyncio.Lock()

    async def health(_request: web.Request) -> web.Response:
        return web.json_response({"status": "ok"})

    async def system_one(request: web.Request) -> web.Response:
        try:
            body = await request.json()
            questions = body["questions"]
            images = [decode_image(url) for url in body.get("images") or []]
        except (ValueError, KeyError, TypeError, AttributeError) as error:
            raise web.HTTPBadRequest(text=str(error)) from error
        async with lock:
            try:
                reply = await asyncio.to_thread(model.system_one, body.get("state"), questions, images or None)
            except ValueError as error:
                raise web.HTTPUnprocessableEntity(text=str(error)) from error
        return web.json_response({"answers": reply["answers"], "usage": reply["usage"]})

    app = web.Application()
    app.router.add_get("/health", health)
    app.router.add_post(SYSTEM_ONE_PATH, system_one)
    return app


def decode_image(url: str) -> Image.Image:
    """Raises ValueError for anything but a base64 image data URL."""
    header, separator, data = url.partition(",")
    if not separator or not header.startswith("data:image/") or not header.endswith(";base64"):
        raise ValueError("an image is a base64 data URL such as data:image/jpeg;base64,...")
    try:
        return Image.open(io.BytesIO(base64.b64decode(data, validate=True))).convert("RGB")
    except (binascii.Error, UnidentifiedImageError) as error:
        raise ValueError(f"unreadable image: {error}") from error


def load_model(name: str, revision: str) -> Any:
    import torch
    from transformers import AutoModel

    device = "cuda" if torch.cuda.is_available() else "mps" if torch.backends.mps.is_available() else "cpu"
    dtype = torch.float32 if device == "cpu" else torch.bfloat16
    logger.info("loading %s on %s", name, device)
    model = AutoModel.from_pretrained(name, revision=revision, trust_remote_code=True, dtype=dtype).to(device)
    model.eval()
    return model


def main() -> None:
    parser = argparse.ArgumentParser(description="Serve a System One decision model such as d1-2B over HTTP")
    parser.add_argument("--model", default=DEFAULT_MODEL, help="a Hugging Face model id or a local directory")
    parser.add_argument("--revision", default=DEFAULT_REVISION, help="the commit of a Hub model id")
    parser.add_argument("--host", default="127.0.0.1")
    parser.add_argument("--port", type=int, default=8000)
    args = parser.parse_args()
    logging.basicConfig(level=logging.INFO)
    web.run_app(create_app(load_model(args.model, args.revision)), host=args.host, port=args.port)


if __name__ == "__main__":
    main()
