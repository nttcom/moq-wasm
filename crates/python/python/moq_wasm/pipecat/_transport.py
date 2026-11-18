import asyncio
import logging
from collections.abc import Callable

from pipecat.frames.frames import (
    CancelFrame,
    EndFrame,
    OutputAudioRawFrame,
    OutputImageRawFrame,
    StartFrame,
    UserAudioRawFrame,
    UserImageRawFrame,
)
from pipecat.transports.base_input import BaseInputTransport
from pipecat.transports.base_output import BaseOutputTransport
from pipecat.transports.base_transport import BaseTransport, TransportParams

from moq_wasm._session import Session
from moq_wasm.media import Catalog, CatalogTrack, RemoteBroadcast
from moq_wasm.pipecat._participant import ReceiveOptions, receive_participant
from moq_wasm.pipecat._room import Room, RoomOptions

DEFAULT_VIDEO_OUT_CODEC = "H264"

logger = logging.getLogger("moq_wasm")


class MoqWasmInputParams(TransportParams):
    """`TransportParams` with how a participant's video is received.

    Parameters:
        video_in_max_fps: How many pictures a second, at most, each participant's video yields.
        video_in_track: Picks the video track of a participant's catalog; by default the LOC
            video track with the lowest bitrate that PyAV decodes.
    """

    video_in_max_fps: float = 10.0
    video_in_track: Callable[[Catalog], CatalogTrack | None] | None = None


class MoqWasmParams(MoqWasmInputParams):
    """`MoqWasmInputParams` with the room the transport joins.

    Parameters:
        room: The namespace whose broadcasts one level below are the participants.
        participant_id: The transport's own broadcast is `[*room, participant_id]`.
    """

    room: list[str]
    participant_id: str = "bot"


def _receive_options(params: MoqWasmInputParams) -> ReceiveOptions:
    return ReceiveOptions(
        audio_in=params.audio_in_enabled,
        video_in=params.video_in_enabled,
        video_in_max_fps=params.video_in_max_fps,
        video_in_track=params.video_in_track,
    )


class _ReceivingInput(BaseInputTransport):
    """Pushes received audio as `UserAudioRawFrame` and pictures as `UserImageRawFrame`, with the
    participant id as `user_id`."""

    def audio_in_sample_rate(self) -> int:
        return self.sample_rate

    async def push_audio(self, pcm: bytes, sample_rate: int, participant_id: str) -> None:
        # Drops audio that arrives before set_transport_ready creates the audio queue or after stop.
        if self._audio_task is None:
            return
        await self.push_audio_frame(
            UserAudioRawFrame(audio=pcm, sample_rate=sample_rate, num_channels=1, user_id=participant_id)
        )

    async def push_picture(self, rgb: bytes, size: tuple[int, int], participant_id: str) -> None:
        await self.push_video_frame(UserImageRawFrame(image=rgb, size=size, format="RGB", user_id=participant_id))


class MoqWasmInputTransport(_ReceivingInput):
    """The input of `MoqWasmTransport`: every participant of its room."""

    def __init__(self, room: Room, params: MoqWasmParams, **kwargs):
        super().__init__(params, **kwargs)
        self._room = room

    async def start(self, frame: StartFrame):
        await super().start(frame)
        await self._room.start(self)
        await self.set_transport_ready(frame)

    async def stop(self, frame: EndFrame):
        await super().stop(frame)
        await self._room.stop(self)

    async def cancel(self, frame: CancelFrame):
        await super().cancel(frame)
        await self._room.stop(self)


class MoqWasmParticipantInput(_ReceivingInput):
    """The input of one participant's broadcast, for a pipeline per participant (a voice activity
    detector must hear one speaker). Picks and decodes the tracks as `MoqWasmTransport` does, from
    the `StartFrame` until the `EndFrame`, the `CancelFrame` or the end of the catalog. Publishes
    nothing: follow the room with `watch_broadcasts` and publish with `create_broadcast`.

    Event handlers: `on_track_subscribed` (takes a `MoqWasmTrack`). A failed track, such as one
    PyAV cannot decode, is pushed upstream as a non-fatal `ErrorFrame`, which reaches pipecat's
    `on_error`."""

    def __init__(self, broadcast: RemoteBroadcast, params: MoqWasmInputParams, **kwargs):
        super().__init__(params, **kwargs)
        self._broadcast = broadcast
        self._options = _receive_options(params)
        self._receiving: asyncio.Task | None = None
        self._register_event_handler("on_track_subscribed")

    async def start(self, frame: StartFrame):
        await super().start(frame)
        await self.set_transport_ready(frame)
        if self._receiving is None:
            self._receiving = asyncio.create_task(receive_participant(self._broadcast, self._options, self))
            self._receiving.add_done_callback(_log_failure)

    async def stop(self, frame: EndFrame):
        await self._stop_receiving()
        await super().stop(frame)

    async def cancel(self, frame: CancelFrame):
        await self._stop_receiving()
        await super().cancel(frame)

    async def event(self, name: str, *args: object) -> None:
        if name == "on_error":
            message, error = args
            await self.push_error(message, error)
        else:
            await self._call_event_handler(name, *args)

    async def _stop_receiving(self) -> None:
        receiving, self._receiving = self._receiving, None
        if receiving is not None:
            receiving.cancel()
            await asyncio.gather(receiving, return_exceptions=True)


def _log_failure(receiving: asyncio.Task) -> None:
    if not receiving.cancelled() and receiving.exception() is not None:
        logger.error("receiving a participant failed", exc_info=receiving.exception())


class MoqWasmOutputTransport(BaseOutputTransport):
    """Publishes `OutputAudioRawFrame` as Opus and `OutputImageRawFrame` as `video_out_codec` in the
    transport's broadcast, with its MSF catalog."""

    def __init__(self, room: Room, params: MoqWasmParams, **kwargs):
        super().__init__(params, **kwargs)
        self._room = room

    async def start(self, frame: StartFrame):
        await super().start(frame)
        await self._room.start(self)
        await self.set_transport_ready(frame)

    async def stop(self, frame: EndFrame):
        await super().stop(frame)
        await self._room.stop(self)

    async def cancel(self, frame: CancelFrame):
        await super().cancel(frame)
        await self._room.stop(self)

    async def write_audio_frame(self, frame: OutputAudioRawFrame) -> bool:
        await self._room.write_audio(frame.audio, frame.sample_rate)
        return True

    async def write_video_frame(self, frame: OutputImageRawFrame) -> bool:
        await self._room.write_picture(frame.image, frame.size, frame.format or "RGB")
        return True


class MoqWasmTransport(BaseTransport):
    """Takes the place of pipecat's `MOQTransport` on a `moq_wasm.Session`, which owns the connection
    and tokens. Every broadcast one level under `params.room` is a participant.

    Event handlers, as `MOQTransport`'s: `on_connected` and `on_disconnected` (the transport started
    and stopped), `on_client_connected` and `on_client_disconnected` (a participant's broadcast was
    published and withdrawn; they take the participant id), `on_track_subscribed` (takes a
    `MoqWasmTrack`) and `on_error` (takes a message and the exception)."""

    def __init__(
        self,
        session: Session,
        params: MoqWasmParams,
        input_name: str | None = None,
        output_name: str | None = None,
    ):
        super().__init__(input_name=input_name, output_name=output_name)
        self._params = params
        options = RoomOptions(
            room=params.room,
            participant_id=params.participant_id,
            receive=_receive_options(params),
            audio_out=params.audio_out_enabled,
            video_out=params.video_out_enabled,
            video_out_codec=params.video_out_codec or DEFAULT_VIDEO_OUT_CODEC,
            video_out_size=(params.video_out_width, params.video_out_height),
            video_out_framerate=params.video_out_framerate,
        )
        self._room = Room(session, options, self)
        self._input: MoqWasmInputTransport | None = None
        self._output: MoqWasmOutputTransport | None = None
        for event in (
            "on_connected",
            "on_disconnected",
            "on_client_connected",
            "on_client_disconnected",
            "on_track_subscribed",
            "on_error",
        ):
            self._register_event_handler(event)

    def input(self) -> MoqWasmInputTransport:
        if self._input is None:
            self._input = MoqWasmInputTransport(self._room, self._params, name=self._input_name)
        return self._input

    def output(self) -> MoqWasmOutputTransport:
        if self._output is None:
            self._output = MoqWasmOutputTransport(self._room, self._params, name=self._output_name)
        return self._output

    def audio_in_sample_rate(self) -> int:
        return self.input().audio_in_sample_rate()

    async def push_audio(self, pcm: bytes, sample_rate: int, participant_id: str) -> None:
        await self.input().push_audio(pcm, sample_rate, participant_id)

    async def push_picture(self, rgb: bytes, size: tuple[int, int], participant_id: str) -> None:
        await self.input().push_picture(rgb, size, participant_id)

    async def event(self, name: str, *args: object) -> None:
        await self._call_event_handler(name, *args)
