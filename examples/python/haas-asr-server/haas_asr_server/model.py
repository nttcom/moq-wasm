import asyncio
import concurrent.futures

import haas_asr
import numpy as np
import torch

from haas_asr_server.server import SAMPLE_RATE


class HaasTranscriber:
    """A Haas model that runs every call on one worker thread: haas-asr measured a call from a new
    thread each time as 2-3 times slower. Concurrent requests queue behind each other."""

    def __init__(self, model: str):
        self._executor = concurrent.futures.ThreadPoolExecutor(max_workers=1, thread_name_prefix="haas-model")
        self._model = self._executor.submit(haas_asr.load_model, model).result()
        # The first call on the GPU compiles kernels, so a warm-up keeps that off the first utterance.
        self._executor.submit(self._transcribe, np.zeros(SAMPLE_RATE, dtype=np.float32)).result()

    async def transcribe(self, audio: np.ndarray) -> str:
        return await asyncio.get_running_loop().run_in_executor(self._executor, self._transcribe, audio)

    def _transcribe(self, audio: np.ndarray) -> str:
        # The bot joins the segments that arrive while this is busy, so audio can pass the 30 s the model
        # was trained on; transcribe_longform splits it.
        with torch.inference_mode():
            return haas_asr.transcribe_longform(self._model, audio).strip()
