from moq_wasm.pipecat import MoqWasmTrackObject
from pipecat.frames.frames import OutputTransportMessageFrame
from pipecat.pipeline.worker import PipelineWorker


class PipelineTrack:
    """Writes one of a `MoqWasmTransport`'s `data_out_tracks` from outside its pipeline, as a
    `TrackWriter` does, by queueing the objects into the pipeline for the transport's output."""

    def __init__(self, worker: PipelineWorker, track: str):
        self._worker = worker
        self._track = track

    async def write(self, payload: bytes, *, new_group: bool = False) -> None:
        await self._worker.queue_frame(OutputTransportMessageFrame(message=MoqWasmTrackObject(self._track, payload, new_group)))
