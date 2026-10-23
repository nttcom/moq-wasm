from __future__ import annotations

import asyncio
from collections.abc import Callable
from dataclasses import dataclass, field
from typing import TYPE_CHECKING

import numpy as np
from loguru import logger

from moq_meeting_transcriber.audio import SAMPLE_RATE

if TYPE_CHECKING:
    from transformers import Pipeline

MODEL = "ibm-granite/granite-speech-5.0-470m-turboctc"


def load_granite() -> Pipeline:
    # Importing torch and transformers takes over 10 s and about 300 MB, which a bot whose rooms use
    # only Whisper would spend on every start before it listens.
    import torch
    from transformers import pipeline

    logger.info(f"loading {MODEL}")
    # On the CPU, the weights' own bfloat16 took about four times as long per utterance as float32.
    model = pipeline("automatic-speech-recognition", model=MODEL, device="cpu", dtype=torch.float32)
    logger.info(f"{MODEL} loaded")
    return model


@dataclass
class SharedGranite:
    """The process's one Granite Speech 5.0 470m on the CPU, which the participants' pipelines take
    turns on. It loads when a room first picks it, so a bot whose rooms use Whisper does not hold it.
    The model takes no language: its CTC head writes what it hears."""

    load: Callable[[], Pipeline] = load_granite
    name: str = "granite-speech-5-470m"
    lock: asyncio.Lock = field(default_factory=asyncio.Lock)
    _model: asyncio.Future[Pipeline] | None = None

    async def ready(self) -> None:
        await asyncio.shield(self._loaded())

    async def recognize(self, pcm: bytes) -> str:
        model = await asyncio.shield(self._loaded())
        samples = np.frombuffer(pcm, dtype=np.int16).astype(np.float32) / 32768.0
        async with self.lock:
            result = await asyncio.to_thread(model, {"raw": samples, "sampling_rate": SAMPLE_RATE})
        return result["text"]

    def _loaded(self) -> asyncio.Future[Pipeline]:
        if self._model is None:
            self._model = asyncio.ensure_future(asyncio.to_thread(self.load))
        return self._model
