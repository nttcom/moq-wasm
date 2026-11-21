import asyncio
import logging
from collections.abc import Callable

from pipecat.audio.filters.base_audio_filter import BaseAudioFilter
from pipecat.frames.frames import (
    CancelFrame,
    EndFrame,
    Frame,
    InputTransportMessageFrame,
    InterruptionFrame,
    OutputAudioRawFrame,
    OutputImageRawFrame,
    OutputTransportMessageFrame,
    OutputTransportMessageUrgentFrame,
    SpriteFrame,
    StartFrame,
    UserAudioRawFrame,
    UserImageRawFrame,
    UserImageRequestFrame,
)
from pipecat.processors.frame_processor import FrameDirection, FrameProcessor, FrameProcessorSetup
from pipecat.transports.base_input import BaseInputTransport
from pipecat.transports.base_output import BaseOutputTransport
from pipecat.transports.base_transport import BaseTransport, TransportParams

from moq_wasm._once import Once
from moq_wasm._session import Session
from moq_wasm.media import Catalog, CatalogTrack, RemoteBroadcast
from moq_wasm.pipecat._participant import (
    CAMERA_SOURCE,
    MoqWasmTrackMessage,
    ParticipantReceiver,
    ReceivedPicture,
    ReceiveOptions,
    departure_error,
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
        broadcast: The namespace of the transport's own broadcast, such as `[*room, "bots",
            "chat-moderation"]`, the namespace the bot service lets its bots publish at.
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


class _OneRun(FrameProcessor):
    """A processor that serves one pipeline run. pipecat's processor keeps what its run left, such
    as the frames still queued as a `CancelFrame` came or the unusable state an error set, so one
    run again would carry them into the next pipeline. Once cleaned up, its `setup` raises, which
    pipecat reports as an `ErrorFrame` treated as permanent, and its `start` does nothing."""

    # Whether the processor ran in a pipeline that cleaned it up.
    ran = False
    # Set once pipecat set the processor up.
    _set_up = False

    def _audio_filter(self) -> BaseAudioFilter | None:
        """The audio filter pipecat's `setup` starts and its `cleanup` stops."""
        return None

    async def setup(self, setup: FrameProcessorSetup):
        # A StopFrame leaves the processor running, uncleaned, and the pipeline that resumes it sets
        # it up again, where pipecat starts its audio filter again at that pipeline's sample rate:
        # the filter is stopped first, so each start has its stop.
        audio_filter = self._audio_filter()
        if self._set_up and not self.ran and audio_filter is not None:
            await audio_filter.stop()
        await super().setup(setup)
        self._set_up = True
        if self.ran:
            raise RuntimeError(f"{self} ran in a pipeline already: give each pipeline new processors")

    async def cleanup(self):
        try:
            await super().cleanup()
        finally:
            self.ran = True


class _ReceivingInput(_OneRun, BaseInputTransport):
    """Pushes received audio as `UserAudioRawFrame`, pictures as `UserImageRawFrame` and data
    objects as `InputTransportMessageFrame`, with the participant id as `user_id`, and answers
    `UserImageRequestFrame`s. A picture carries the `Location` of its object in
    `metadata["location"]` and its video source in `transport_source`. Paused by a `StopFrame`, it
    drops the audio, pictures and messages it receives until the next `StartFrame`."""

    def audio_in_sample_rate(self) -> int:
        return self.sample_rate

    def _audio_filter(self) -> BaseAudioFilter | None:
        return self._params.audio_in_filter

    async def push_audio(self, pcm: bytes, sample_rate: int, participant_id: str) -> None:
        # Drops audio that arrives before set_transport_ready creates the audio queue, or after
        # stop.
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
        # As `push_video_frame` does: a paused input drops the message.
        if self._paused:
            return
        await self.push_frame(InputTransportMessageFrame(message=message))

    async def answer_image_requests(self, requests: list[UserImageRequestFrame], error: str) -> None:
        await self._answer_image_requests(requests, error)

    async def request_image(self, request: UserImageRequestFrame) -> None:
        raise NotImplementedError

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
    """The input of `MoqWasmTransport`: every participant of its room. It serves one pipeline run:
    the next pipeline takes a new input from `MoqWasmTransport.input`."""

    def __init__(self, room: Room, params: MoqWasmParams, **kwargs):
        super().__init__(params, **kwargs)
        self._room = room
        # The leaving of the room for an EndFrame, before the frame is passed on or in `stop`.
        self._leaving_at_end: Once[None] = Once()

    async def start(self, frame: StartFrame):
        await super().start(frame)
        if self.ran:
            return
        if await _open_room(self, self._room):
            await self.set_transport_ready(frame)

    async def process_frame(self, frame: Frame, direction: FrameDirection):
        if isinstance(frame, EndFrame):
            # pipecat's input passes an EndFrame on before it stops, and the pipeline ends once
            # the frame came out of it: the room is left first, so the failure of a withdrawal this
            # input makes, as the room's last processor, goes upstream while the pipeline runs.
            await self._leave_at_end()
        await super().process_frame(frame, direction)

    async def stop(self, frame: EndFrame):
        await super().stop(frame)
        await self._leave_at_end()

    async def _leave_at_end(self) -> None:
        await self._leaving_at_end.run(lambda: _leave_room(self, self._room))

    async def cancel(self, frame: CancelFrame):
        await super().cancel(frame)
        await _leave_room(self, self._room)

    async def cleanup(self):
        # pipecat cleans up every processor, also one that no EndFrame or CancelFrame reached; the
        # room is left even when pipecat's cleanup fails.
        try:
            await super().cleanup()
        finally:
            await self._room.stop(self)

    async def request_image(self, request: UserImageRequestFrame) -> None:
        await self._room.request_image(request)


class MoqWasmParticipantInput(_ReceivingInput):
    """The input of one participant's broadcast, for a pipeline per participant (a voice activity
    detector must hear one speaker). Picks and decodes the tracks as `MoqWasmTransport` does, from
    the `StartFrame` until the `EndFrame`, the `CancelFrame` or its `cleanup`, each of which returns
    once every track is unsubscribed and the image requests waiting are answered, even after pipecat
    cancelled an earlier one. Publishes nothing: follow the room with `watch_broadcasts` and
    publish with `create_broadcast`.

    It serves one pipeline run: create a new one for each pipeline. Set up again once its pipeline
    cleaned it up, it pushes an `ErrorFrame` treated as permanent and receives nothing. A
    `StopFrame` does not end its run, as pipecat keeps a stopped processor running: it keeps its
    subscriptions, drops what they bring until the next `StartFrame`, and then resumes them.

    Event handlers: `on_track_subscribed` (takes a `MoqWasmTrack`). A failed track, such as one
    PyAV cannot decode, is pushed upstream as a non-fatal `ErrorFrame`, which reaches pipecat's
    `on_error`."""

    def __init__(self, broadcast: RemoteBroadcast, params: MoqWasmInputParams, **kwargs):
        super().__init__(params, **kwargs)
        self._participant_id = broadcast.namespace[-1]
        # None once the receiving ended for good: an input the application still holds once it
        # stopped keeps neither the participant's broadcast, nor through it the session, nor its
        # catalog.
        self._receiver: ParticipantReceiver | None = ParticipantReceiver(broadcast, _receive_options(params), self)
        # Kept until its teardown finished, which `_stopping` runs once for the stop, the cancel and
        # the cleanup: a caller pipecat cancels leaves it running for the others to wait for.
        self._receiving: asyncio.Task | None = None
        self._stopping: Once[None] = Once()
        self._register_event_handler("on_track_subscribed")

    async def start(self, frame: StartFrame):
        await super().start(frame)
        if self.ran:
            return
        await self.set_transport_ready(frame)
        receiver = self._receiver
        if receiver is None:
            return
        # A StopFrame leaves the input receiving, as pipecat keeps a stopped processor running, and
        # pausing it drops what it receives: the next StartFrame resumes that same receiving.
        if self._receiving is None and not self._stopping.started:
            self._receiving = asyncio.create_task(receiver.run())
            self._receiving.add_done_callback(_log_failure)

    async def request_image(self, request: UserImageRequestFrame) -> None:
        """`ParticipantReceiver.request_image`. A request for another participant, or made once the
        receiving ended, is answered with an error at once, and the call, which the input's frame
        processing awaits, returns once its `result_callback` did."""
        if request.user_id != self._participant_id:
            await self.answer_image_requests([request], f"no participant {request.user_id}")
            return
        if self._receiver is None:
            await self.answer_image_requests([request], departure_error(self._participant_id))
            return
        await self._receiver.request_image(request)

    async def stop(self, frame: EndFrame):
        await self._stop_receiving()
        await super().stop(frame)

    async def cancel(self, frame: CancelFrame):
        await self._stop_receiving()
        await super().cancel(frame)

    async def cleanup(self):
        # pipecat cleans up every processor, also one that no EndFrame or CancelFrame reached; the
        # tracks are unsubscribed even when pipecat's cleanup fails, as an audio filter's `stop`
        # may.
        try:
            await super().cleanup()
        finally:
            await self._stop_receiving()

    async def event(self, name: str, *args: object) -> None:
        if name == "on_error":
            message, error = args
            assert isinstance(message, str) and isinstance(error, Exception)
            await self.push_error(message, error)
        else:
            await self._call_event_handler(name, *args)

    async def _stop_receiving(self) -> None:
        """Cancels the receiving once and returns once its teardown finished: every track
        unsubscribed and the image requests still waiting answered. Cancelling the caller, as
        pipecat's cleanup cancels a stop in progress, leaves the teardown running."""
        await self._stopping.run(self._end_receiving)

    async def _end_receiving(self) -> None:
        receiving = self._receiving
        if receiving is not None:
            # The receiving returns only once its teardown finished, also one the session's end
            # started before this cancels it.
            receiving.cancel()
            await asyncio.wait([receiving])
            # Its cancellation's traceback keeps the receiving's locals: an input the application
            # still holds once it stopped keeps none of them.
            self._receiving = None
        self._receiver = None


async def _open_room(processor: FrameProcessor, room: Room) -> bool:
    """Starts `room` for `processor`. A room that cannot open leaves the processor unable to do its
    job: it pushes an `ErrorFrame` treated as permanent, and the pipeline worker's
    `processor_unusable_policy` decides whether the pipeline ends."""
    try:
        await room.start(processor)
    except Exception as error:
        await processor.push_error(f"cannot open the room: {error}", error, force_treat_as_permanent=True)
        return False
    return True


async def _leave_room(processor: FrameProcessor, room: Room) -> None:
    """Stops `room` for `processor` as an `EndFrame` or a `CancelFrame` reaches it, before the
    frame is passed on. A teardown that fails, as when the relay refuses to withdraw the
    broadcast, is pushed upstream as a non-fatal `ErrorFrame`, which reaches the pipeline's
    source before the frame comes out of the pipeline and ends it."""
    try:
        await room.stop(processor)
    except Exception as error:
        await processor.push_error(f"cannot leave the room: {error}", error)


def _log_failure(receiving: asyncio.Task) -> None:
    if not receiving.cancelled() and receiving.exception() is not None:
        logger.error("receiving a participant failed", exc_info=receiving.exception())


class MoqWasmOutputTransport(_OneRun, BaseOutputTransport):
    """Publishes `OutputAudioRawFrame` as Opus and `OutputImageRawFrame` as `video_out_codec` in the
    transport's broadcast, with its MSF catalog, and writes the `MoqWasmTrackObject` or
    `MoqWasmEventRecord` an `OutputTransportMessageFrame` carries. It serves one pipeline run: the
    next pipeline takes a new output from `MoqWasmTransport.output`."""

    def __init__(self, room: Room, params: MoqWasmParams, **kwargs):
        super().__init__(params, **kwargs)
        self._room = room

    def _audio_filter(self) -> BaseAudioFilter | None:
        return self._params.audio_out_filter

    async def start(self, frame: StartFrame):
        await super().start(frame)
        if self.ran:
            return
        await self._end_stopped_senders()
        if await _open_room(self, self._room):
            await self.set_transport_ready(frame)

    async def _end_stopped_senders(self) -> None:
        """Ends the media senders a `StopFrame` left running: pipecat keeps a stopped processor
        running, and `set_transport_ready` starts new senders in their place at the next
        `StartFrame`. The audio queued before the `StopFrame` was written as it went through."""
        senders = list(self._media_senders.values())
        self._media_senders.clear()
        for sender in senders:
            await sender.cleanup()

    async def stop(self, frame: EndFrame):
        try:
            await super().stop(frame)
        finally:
            await _leave_room(self, self._room)

    async def cancel(self, frame: CancelFrame):
        try:
            await super().cancel(frame)
        finally:
            await _leave_room(self, self._room)

    async def cleanup(self):
        # pipecat cleans up every processor, also one that no EndFrame or CancelFrame reached; the
        # room is left even when pipecat's cleanup fails.
        try:
            await super().cleanup()
        finally:
            await self._room.stop(self)

    async def _handle_frame(self, frame: Frame):
        # pipecat drops a frame for which it has no media sender, and an output whose room never
        # opened has none, which pipecat's default ProcessorUnusablePolicy.CONTINUE keeps running:
        # such a frame goes on, so a StopFrame still stops the pipeline, but the audio and pictures
        # the output did not write do not, and an InterruptionFrame went on already.
        if frame.transport_destination not in self._media_senders and not isinstance(
            frame, (InterruptionFrame, OutputAudioRawFrame, OutputImageRawFrame, SpriteFrame)
        ):
            await self.push_frame(frame)
            return
        await super()._handle_frame(frame)

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
    participant for `empty_timeout` seconds).

    A room that cannot open, as when the relay refuses the broadcast or the room's
    SUBSCRIBE_NAMESPACE, leaves the input and the output unusable: each pushes an `ErrorFrame`
    treated as permanent. pipecat's `PipelineWorker` keeps running by default
    (`ProcessorUnusablePolicy.CONTINUE`), so build it with
    `processor_unusable_policy=ProcessorUnusablePolicy.CANCEL` (or `END`) to end the pipeline then.
    An output that keeps running without a media sender passes on the frames other than audio and
    pictures, which pipecat would route to that sender, so a `StopFrame` still stops the pipeline.
    A room that cannot be left at an `EndFrame` or a `CancelFrame` is reported
    with a non-fatal `ErrorFrame`, and the frame goes on, so the pipeline ends.

    A processor serves one pipeline run: once pipecat cleaned up the input or the output at the end
    of its pipeline, `input()` or `output()` returns a new one, which opens the room again, so
    build each pipeline from them rather than from the processors of an earlier one. A `StopFrame`
    does not end the run: pipecat keeps the stopped processors running, in the room, and the next
    pipeline's `StartFrame` resumes them, the output's media senders replaced by new ones."""

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
        """The input of the pipeline being built: the same one until it ran, then a new one."""
        if self._input is None or self._input.ran:
            self._input = MoqWasmInputTransport(self._room, self._params, name=self._input_name)
        return self._input

    def output(self) -> MoqWasmOutputTransport:
        """The output of the pipeline being built: the same one until it ran, then a new one."""
        if self._output is None or self._output.ran:
            self._output = MoqWasmOutputTransport(self._room, self._params, name=self._output_name)
        return self._output

    def _receiving_input(self) -> MoqWasmInputTransport:
        """The input the room's participants go to: the latest one, even once it ran, so a
        participant's media arriving as its pipeline ends makes no input for the next pipeline."""
        return self._input if self._input is not None else self.input()

    def audio_in_sample_rate(self) -> int:
        return self._receiving_input().audio_in_sample_rate()

    async def push_audio(self, pcm: bytes, sample_rate: int, participant_id: str) -> None:
        await self._receiving_input().push_audio(pcm, sample_rate, participant_id)

    async def push_picture(self, picture: ReceivedPicture) -> None:
        await self._receiving_input().push_picture(picture)

    async def push_message(self, message: MoqWasmTrackMessage) -> None:
        await self._receiving_input().push_message(message)

    async def answer_image_requests(self, requests: list[UserImageRequestFrame], error: str) -> None:
        await self._receiving_input().answer_image_requests(requests, error)

    async def event(self, name: str, *args: object) -> None:
        await self._call_event_handler(name, *args)
