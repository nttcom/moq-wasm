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
from moq_wasm.media import Catalog, CatalogTrack
from moq_wasm.pipecat._room import Room, RoomOptions

DEFAULT_VIDEO_OUT_CODEC = "H264"


class MoqWasmParams(TransportParams):
    """`TransportParams` with the room the transport joins.

    Parameters:
        room: The namespace whose broadcasts one level below are the participants.
        participant_id: The transport's own broadcast is `[*room, participant_id]`.
        video_in_max_fps: How many pictures a second, at most, each participant's video yields.
        video_in_track: Picks the video track of a participant's catalog; by default the LOC
            video track with the lowest bitrate that PyAV decodes.
    """

    room: list[str]
    participant_id: str = "bot"
    video_in_max_fps: float = 10.0
    video_in_track: Callable[[Catalog], CatalogTrack | None] | None = None


class MoqWasmInputTransport(BaseInputTransport):
    """Pushes each participant's audio as `UserAudioRawFrame` and pictures as `UserImageRawFrame`,
    with the participant id as `user_id`."""

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

    async def push_received_audio(self, pcm: bytes, sample_rate: int, participant_id: str) -> None:
        # The room follows participants before set_transport_ready creates the audio queue; audio
        # before then, and after stop, is dropped.
        if self._audio_task is None:
            return
        await self.push_audio_frame(
            UserAudioRawFrame(audio=pcm, sample_rate=sample_rate, num_channels=1, user_id=participant_id)
        )

    async def push_received_picture(self, rgb: bytes, size: tuple[int, int], participant_id: str) -> None:
        await self.push_video_frame(UserImageRawFrame(image=rgb, size=size, format="RGB", user_id=participant_id))


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
    """Takes the place of pipecat's `MOQTransport` on a `moq_wasm.Session`, which owns reconnection
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
            audio_in=params.audio_in_enabled,
            video_in=params.video_in_enabled,
            video_in_max_fps=params.video_in_max_fps,
            video_in_track=params.video_in_track,
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
        return self.input().sample_rate

    async def push_audio(self, pcm: bytes, sample_rate: int, participant_id: str) -> None:
        await self.input().push_received_audio(pcm, sample_rate, participant_id)

    async def push_picture(self, rgb: bytes, size: tuple[int, int], participant_id: str) -> None:
        await self.input().push_received_picture(rgb, size, participant_id)

    async def event(self, name: str, *args: object) -> None:
        await self._call_event_handler(name, *args)
