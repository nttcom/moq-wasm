import asyncio
import logging
from collections.abc import Callable

from pipecat.frames.frames import (
    CancelFrame,
    EndFrame,
    Frame,
    InputTransportMessageFrame,
    OutputAudioRawFrame,
    OutputImageRawFrame,
    OutputTransportMessageFrame,
    OutputTransportMessageUrgentFrame,
    StartFrame,
    UserAudioRawFrame,
    UserImageRawFrame,
    UserImageRequestFrame,
)
from pipecat.processors.frame_processor import FrameDirection, FrameProcessor
from pipecat.transports.base_input import BaseInputTransport
from pipecat.transports.base_output import BaseOutputTransport
from pipecat.transports.base_transport import BaseTransport, TransportParams

from moq_wasm._session import Session
from moq_wasm.media import Catalog, CatalogTrack, RemoteBroadcast
from moq_wasm.pipecat._participant import (
    CAMERA_SOURCE,
    MoqWasmTrackMessage,
    ParticipantReceiver,
    ReceivedPicture,
    ReceiveOptions,
)
from moq_wasm.pipecat._room import Room, RoomOptions

DEFAULT_VIDEO_OUT_CODEC = "H264"

logger = logging.getLogger("moq_wasm")


class MoqWasmInputParams(TransportParams):
    """`TransportParams` with how a participant's broadcast is received.

    A video source names a track of each participant's catalog: `"camera"` is the track
    `video_in_track` picks (by default the decodable LOC video track with the lowest bitrate); any
    other source is the video track of that name. As in pipecat, `video_in_enabled` receives the
    sources `video_in_sources` lists as each participant joins, and others only when asked for, by
    `MoqWasmTransport.capture_participant_video` or a `UserImageRequestFrame`.

    Parameters:
        video_in_track: Picks the `"camera"` video track of a participant's catalog.
        data_in_tracks: Tracks of each participant whose objects are pushed as
            `InputTransportMessageFrame`s carrying a `MoqWasmTrackMessage`.
    """

    video_in_track: Callable[[Catalog], CatalogTrack | None] | None = None
    data_in_tracks: list[str] = []


class MoqWasmParams(MoqWasmInputParams):
    """`MoqWasmInputParams` with the room the transport joins and the tracks it publishes.

    Parameters:
        room: The namespace whose broadcasts one level below are the participants.
        participant_id: Labels the transport's media source, and names its broadcast
            `[*room, participant_id]` unless `broadcast` does.
        broadcast: The namespace of the transport's own broadcast, such as `[*room, "bot", "voice"]`
            beside the bot's other broadcasts.
        data_out_tracks: Tracks of the transport's broadcast, by name, with the seconds each keeps
            its objects for FETCH; an `OutputTransportMessageFrame` carrying a
            `MoqWasmTrackObject` writes one.
        event_timelines_out: Event timeline tracks of the transport's broadcast, by name, with
            the seconds each keeps its records; an `OutputTransportMessageFrame` carrying a
            `MoqWasmEventRecord` appends one.
        empty_timeout: Seconds without any participant after which `on_room_empty` fires and
            the transport stops following the room.
    """

    room: list[str]
    participant_id: str = "bot"
    broadcast: list[str] | None = None
    data_out_tracks: dict[str, float] = {}
    event_timelines_out: dict[str, float] = {}
    empty_timeout: float | None = None


def _receive_options(params: MoqWasmInputParams) -> ReceiveOptions:
    sources = {
        name: None if source.on_request_only else source.framerate for name, source in params.video_in_sources.items()
    }
    return ReceiveOptions(
        audio_in=params.audio_in_enabled,
        video_sources=sources,
        video_in_track=params.video_in_track,
        data_tracks=list(params.data_in_tracks),
        video_in=params.video_in_enabled,
    )


class _ReceivingInput(BaseInputTransport):
    """Pushes received audio as `UserAudioRawFrame`, pictures as `UserImageRawFrame` and data
    objects as `InputTransportMessageFrame`, with the participant id as `user_id`, and answers
    `UserImageRequestFrame`s. A picture carries the `Location` of its object in
    `metadata["location"]` and its video source in `transport_source`."""

    def audio_in_sample_rate(self) -> int:
        return self.sample_rate

    async def push_audio(self, pcm: bytes, sample_rate: int, participant_id: str) -> None:
        # Drops audio that arrives before set_transport_ready creates the audio queue or after stop.
        if self._audio_task is None:
            return
        await self.push_audio_frame(
            UserAudioRawFrame(audio=pcm, sample_rate=sample_rate, num_channels=1, user_id=participant_id)
        )

    async def push_picture(self, picture: ReceivedPicture) -> None:
        request = picture.request
        frame = UserImageRawFrame(
            image=picture.rgb,
            size=picture.size,
            format="RGB",
            user_id=picture.participant_id,
            text=request.text if request else None,
            append_to_context=request.append_to_context if request else None,
            request=request,
        )
        frame.transport_source = picture.video_source
        frame.metadata["location"] = picture.location
        await self.push_video_frame(frame)

    async def push_message(self, message: MoqWasmTrackMessage) -> None:
        await self.push_frame(InputTransportMessageFrame(message=message))

    async def answer_image_requests(self, requests: list[UserImageRequestFrame], error: str) -> None:
        await self._answer_image_requests(requests, error)

    def _supports_video_in_source(self, video_source: str) -> bool:
        return True

    async def process_frame(self, frame: Frame, direction: FrameDirection):
        # An input with video answers image requests and does not pass them on; one without video
        # passes them upstream, to the other transport's input in a two-transport pipeline.
        if isinstance(frame, UserImageRequestFrame) and self._params.video_in_enabled:
            await FrameProcessor.process_frame(self, frame, direction)
            await self.request_image(frame)
            return
        await super().process_frame(frame, direction)


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

    async def request_image(self, request: UserImageRequestFrame) -> None:
        await self._room.request_image(request)


class MoqWasmParticipantInput(_ReceivingInput):
    """The input of one participant's broadcast, for a pipeline per participant (a voice activity
    detector must hear one speaker). Picks and decodes the tracks as `MoqWasmTransport` does, from
    the `StartFrame` until the `EndFrame` or the `CancelFrame`. Publishes
    nothing: follow the room with `watch_broadcasts` and publish with `create_broadcast`.

    Event handlers: `on_track_subscribed` (takes a `MoqWasmTrack`). A failed track, such as one
    PyAV cannot decode, is pushed upstream as a non-fatal `ErrorFrame`, which reaches pipecat's
    `on_error`."""

    def __init__(self, broadcast: RemoteBroadcast, params: MoqWasmInputParams, **kwargs):
        super().__init__(params, **kwargs)
        self._receiver = ParticipantReceiver(broadcast, _receive_options(params), self)
        self._receiving: asyncio.Task | None = None
        self._register_event_handler("on_track_subscribed")

    async def start(self, frame: StartFrame):
        await super().start(frame)
        await self.set_transport_ready(frame)
        if self._receiving is None:
            self._receiving = asyncio.create_task(self._receiver.run())
            self._receiving.add_done_callback(_log_failure)

    async def request_image(self, request: UserImageRequestFrame) -> None:
        if request.user_id != self._receiver.participant_id:
            await self.answer_image_requests([request], f"no participant {request.user_id}")
            return
        await self._receiver.request_image(request)

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
    transport's broadcast, with its MSF catalog, and writes the `MoqWasmTrackObject` or
    `MoqWasmEventRecord` an `OutputTransportMessageFrame` carries."""

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

    async def send_message(self, frame: OutputTransportMessageFrame | OutputTransportMessageUrgentFrame):
        # pipecat passes a non-urgent message on by itself; an urgent one this transport does not write
        # goes on to the next processor, such as another transport's output.
        written = await self._room.write_message(frame.message)
        if not written and isinstance(frame, OutputTransportMessageUrgentFrame):
            await self.push_frame(frame)


class MoqWasmTransport(BaseTransport):
    """Takes the place of pipecat's `MOQTransport` on a `moq_wasm.Session`, which owns the connection
    and tokens. Every broadcast one level under `params.room` is a participant.

    Event handlers, as `MOQTransport`'s: `on_connected` and `on_disconnected` (the transport started
    and stopped), `on_client_connected` and `on_client_disconnected` (a participant's broadcast was
    published and withdrawn; they take the participant id), `on_track_subscribed` (takes a
    `MoqWasmTrack`), `on_error` (takes a message and the exception) and `on_room_empty` (no
    participant for `empty_timeout` seconds)."""

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
            broadcast=params.broadcast or [*params.room, params.participant_id],
            receive=_receive_options(params),
            audio_out=params.audio_out_enabled,
            video_out=params.video_out_enabled,
            video_out_codec=params.video_out_codec or DEFAULT_VIDEO_OUT_CODEC,
            video_out_size=(params.video_out_width, params.video_out_height),
            video_out_framerate=params.video_out_framerate,
            data_out_tracks=dict(params.data_out_tracks),
            event_timelines_out=dict(params.event_timelines_out),
            empty_timeout=params.empty_timeout,
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
            "on_room_empty",
        ):
            self._register_event_handler(event)

    async def capture_participant_video(
        self,
        participant_id: str,
        framerate: int | None = 30,
        video_source: str = CAMERA_SOURCE,
        *,
        on_request_only: bool = False,
    ):
        """Receives a video source of a participant in the room at `framerate` pictures a second, or
        every picture when it is None, or only for image requests with `on_request_only`."""
        rate = None if on_request_only else (framerate or float("inf"))
        await self._room.capture(participant_id, video_source, rate)

    async def release_participant_video(self, participant_id: str, video_source: str = CAMERA_SOURCE):
        """Stops receiving a video source of a participant, for a publisher that serves one profile to
        all its subscribers, such as onvif-ingest; the image requests waiting on it get an error. Not
        part of pipecat's API."""
        await self._room.release(participant_id, video_source)

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

    async def push_picture(self, picture: ReceivedPicture) -> None:
        await self.input().push_picture(picture)

    async def push_message(self, message: MoqWasmTrackMessage) -> None:
        await self.input().push_message(message)

    async def answer_image_requests(self, requests: list[UserImageRequestFrame], error: str) -> None:
        await self.input().answer_image_requests(requests, error)

    async def event(self, name: str, *args: object) -> None:
        await self._call_event_handler(name, *args)
