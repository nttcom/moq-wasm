import asyncio
import copy
import logging
from collections.abc import Callable, Mapping
from dataclasses import dataclass, field
from typing import TypeVar

from PIL import Image
from pipecat.audio.filters.base_audio_filter import BaseAudioFilter
from pipecat.frames.frames import (
    AssistantImageRawFrame,
    CancelFrame,
    DataFrame,
    EndFrame,
    Frame,
    InputTransportMessageFrame,
    InterruptionFrame,
    OutputAudioRawFrame,
    OutputImageRawFrame,
    OutputTransportMessageFrame,
    OutputTransportMessageUrgentFrame,
    OutputTransportReadyFrame,
    PipelineFlushFrame,
    SpriteFrame,
    StartFrame,
    StopFrame,
    TTSStoppedFrame,
    UserAudioRawFrame,
    UserImageRawFrame,
    UserImageRequestFrame,
)
from pipecat.processors.frame_processor import FrameDirection, FrameProcessor, FrameProcessorSetup
from pipecat.transports.base_input import BaseInputTransport
from pipecat.transports.base_output import BaseOutputTransport
from pipecat.transports.base_transport import BaseTransport, TransportParams
from pipecat.utils.deprecation import warn_deprecated

from moq_wasm._once import Once
from moq_wasm._session import Session
from moq_wasm.codecs import _checked_picture, _PcmResampler
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
# Seconds an output ending gracefully waits for the still it was given last to be written.
STILL_WRITE_TIMEOUT_SECONDS = 1.0
# Seconds of pictures at `video_out_framerate` a live output queues at most: the oldest are dropped
# past them, as when pictures come faster than the framerate. An output holds as many synchronized
# pictures not yet written at most, and drops those that come past them.
LIVE_VIDEO_QUEUE_SECONDS = 2.0
# Seconds an output ending gracefully waits for its live sender to write the pictures queued, and
# for the synchronized pictures released to be written: two seconds of them at its framerate, and a
# second to spare.
LIVE_VIDEO_DRAIN_TIMEOUT_SECONDS = LIVE_VIDEO_QUEUE_SECONDS + 1.0

logger = logging.getLogger("moq_wasm")

FrameT = TypeVar("FrameT", bound=Frame)
ParamsT = TypeVar("ParamsT", bound=TransportParams)


class MoqWasmInputParams(TransportParams):
    """`TransportParams` with how a participant's broadcast is received.

    A video source names a track of each participant's catalog: `"camera"` is the track
    `video_in_track` picks (by default the decodable LOC video track with the lowest bitrate); any
    other source is the video track of that name, when it is LOC and decodable. As in pipecat,
    `video_in_enabled` receives the sources `video_in_sources` lists as each participant joins, and
    others only when asked for, by `MoqWasmTransport.capture_participant_video` or a
    `UserImageRequestFrame`.

    `audio_in_channels` (1, the default, or 2) is the channel count the participants' audio is
    pushed in. pipecat's stock VAD and turn analyzers (Silero, Smart Turn) read the PCM as mono, so
    they need 1: stereo would end each turn early. A pipeline that needs stereo elsewhere downmixes
    it before them.

    Leave `audio_in_sample_rate` unset and set the rate once, with
    `PipelineParams(audio_in_sample_rate=...)`: the inputs then decode at the pipeline's rate, which
    pipecat also starts its stock analyzers at, such as Silero. An input rate that differs from the
    pipeline's reaches them as audio they misread (8 kHz in a 16 kHz pipeline gives no speech at
    all), so a processor that needs another rate resamples the audio for itself, leaving the
    frames the analyzers read at the pipeline's rate.

    Parameters:
        video_in_track: Picks the `"camera"` video track of a participant's catalog. What it
            raises is reported to `on_error` (as an `ErrorFrame` by `MoqWasmParticipantInput`),
            and the camera track received before stays.
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


def _snapshot(params: ParamsT) -> ParamsT:
    """A copy of `params` with copies of its lists and mappings, as `room` and an `audio_out_mixer`
    by destination, so a processor built with it keeps what it was built with; the objects they
    hold, as the mixers, are shared."""
    copied = params.model_copy()
    for name, value in params:
        if isinstance(value, list):
            setattr(copied, name, list(value))
        elif isinstance(value, Mapping):
            setattr(copied, name, dict(value))
    return copied


def _receive_options(params: MoqWasmInputParams) -> ReceiveOptions:
    """What an input receives. Raises ValueError for input audio that is neither mono nor stereo."""
    if params.audio_in_channels not in (1, 2):
        raise ValueError("moq-wasm's pipecat inputs take mono or stereo input audio")
    sources: dict[str, float | None] = {
        name: None if source.on_request_only else source.framerate for name, source in params.video_in_sources.items()
    }
    return ReceiveOptions(
        audio_in=params.audio_in_enabled,
        video_sources=sources,
        video_in_track=params.video_in_track,
        data_tracks=params.data_in_tracks,
        video_in=params.video_in_enabled,
        audio_in_channels=params.audio_in_channels,
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
            UserAudioRawFrame(
                audio=pcm, sample_rate=sample_rate, num_channels=self._params.audio_in_channels, user_id=participant_id
            )
        )

    async def push_picture(self, picture: ReceivedPicture) -> bool:
        # What `push_video_frame` checks: a paused input, or one without video, drops the picture.
        if not self._params.video_in_enabled or self._paused:
            return False
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
        return True

    async def push_message(self, message: MoqWasmTrackMessage) -> None:
        # As `push_video_frame` does: a paused input drops the message.
        if self._paused:
            return
        await self.push_frame(InputTransportMessageFrame(message=message))

    async def answer_image_requests(self, requests: list[UserImageRequestFrame], error: str) -> None:
        """Answers each request with `error`. A `result_callback` that raises is logged and the
        other requests are answered still, so what gives the answers goes on, as a failed track
        that is reported and received again. That includes a CancelledError the callback raised
        itself, as by awaiting a future another task cancelled; a cancellation of the task giving
        the answers still ends it."""
        for request in requests:
            try:
                await self._answer_image_requests([request], error)
            except (Exception, asyncio.CancelledError) as failure:
                answering = asyncio.current_task()
                if isinstance(failure, asyncio.CancelledError) and (answering is None or answering.cancelling()):
                    raise
                logger.exception("the result_callback of an image request of %s failed", request.user_id)


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

    async def push_frame(self, frame: Frame, direction: FrameDirection = FrameDirection.DOWNSTREAM):
        # pipecat's audio task replaces a frame's audio with what the audio_in_filter returns, which
        # a buffering filter makes longer or shorter, and keeps the number of samples it received.
        if isinstance(frame, UserAudioRawFrame):
            frame.num_frames = len(frame.audio) // (2 * frame.num_channels)
        await super().push_frame(frame, direction)


class MoqWasmInputTransport(_ReceivingInput):
    """The input of `MoqWasmTransport`: every participant of its room. With
    `audio_in_stream_on_start` False when it starts, no participant's audio is subscribed to until
    an `InputTransportStartAudioStreamingFrame`. It serves one pipeline run: the next pipeline
    takes a new input from `MoqWasmTransport.input`."""

    def __init__(self, room: Room, params: MoqWasmParams, **kwargs):
        super().__init__(params, **kwargs)
        self._room = room
        # The leaving of the room for an EndFrame, before the frame is passed on or in `stop`.
        self._leaving_at_end: Once[None] = Once()

    async def start(self, frame: StartFrame):
        await super().start(frame)
        if self.ran:
            return
        if self._params.audio_in_stream_on_start:
            await self._room.start_audio()
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

    async def _start_audio_in_streaming(self):
        await self._room.start_audio()

    async def request_image(self, request: UserImageRequestFrame) -> None:
        await self._room.request_image(request)


class MoqWasmParticipantInput(_ReceivingInput):
    """The input of one participant's broadcast, for a pipeline per participant (a voice activity
    detector must hear one speaker). Picks and decodes the tracks as `MoqWasmTransport` does, from
    the `StartFrame` until the `EndFrame`, the `CancelFrame` or its `cleanup`, each of which returns
    once every track is unsubscribed and the image requests waiting are answered, even after pipecat
    cancelled an earlier one. Publishes nothing: follow the room with `watch_broadcasts` and
    publish with `create_broadcast`. It keeps a copy of `params` and of its lists and mappings:
    assigning its fields or changing those once it is built changes nothing.

    It serves one pipeline run: create a new one for each pipeline. Set up again once its pipeline
    cleaned it up, it pushes an `ErrorFrame` treated as permanent and receives nothing. A
    `StopFrame` does not end its run, as pipecat keeps a stopped processor running: it keeps its
    subscriptions, drops what they bring until the next `StartFrame`, and then resumes them.

    Event handlers: `on_track_subscribed` (takes a `MoqWasmTrack`). A failed track, such as one
    PyAV cannot decode, is pushed upstream as a non-fatal `ErrorFrame`, which reaches pipecat's
    `on_error`; an audio or video track is then received again with a fresh decoder, after a
    delay that doubles with each failure in a row."""

    def __init__(self, broadcast: RemoteBroadcast, params: MoqWasmInputParams, **kwargs):
        params = _snapshot(params)
        super().__init__(params, **kwargs)
        self._participant_id = broadcast.namespace[-1]
        # None once the receiving ended for good: an input the application still holds once it
        # stopped keeps neither the participant's broadcast, nor through it the session, nor its
        # catalog.
        self._receiver: ParticipantReceiver | None = ParticipantReceiver(
            broadcast, _receive_options(params), self, stream_audio=False
        )
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
        if self._params.audio_in_stream_on_start:
            await receiver.start_audio()
        # A StopFrame leaves the input receiving, as pipecat keeps a stopped processor running, and
        # pausing it drops what it receives: the next StartFrame resumes that same receiving.
        if self._receiving is None and not self._stopping.started:
            self._receiving = asyncio.create_task(receiver.run())
            self._receiving.add_done_callback(_log_failure)

    async def _start_audio_in_streaming(self):
        if self._receiver is not None:
            await self._receiver.start_audio()

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
    next pipeline takes a new output from `MoqWasmTransport.output`. Once publishing its media
    failed, which it does for good, an output that writes no data track or event timeline can do
    nothing more: the failure is pushed as an `ErrorFrame` treated as permanent, as a room that
    cannot open is."""

    def __init__(self, room: Room, params: MoqWasmParams, **kwargs):
        super().__init__(params, **kwargs)
        self._room = room
        # Whether media is all the output writes, so that it can do nothing once publishing it
        # failed.
        self._media_only = not (params.data_out_tracks or params.event_timelines_out)
        # Set up with the output's sample rate, which pipecat sets in `setup`.
        self._audio_in: _AudioNormalizer | None = None

    def _audio_filter(self) -> BaseAudioFilter | None:
        return self._params.audio_out_filter

    async def setup(self, setup: FrameProcessorSetup):
        await super().setup(setup)
        self._audio_in = _AudioNormalizer(self.sample_rate, self._params.audio_out_channels)

    async def start(self, frame: StartFrame):
        await super().start(frame)
        if self.ran:
            return
        await self._end_stopped_senders()
        # A sample rate under 100 Hz has no sample in 10 ms: an audio sender would cut empty chunks
        # forever, so the output can write nothing.
        if self._params.audio_out_enabled and self.audio_chunk_size <= 0:
            error = ValueError(f"{self.sample_rate} Hz audio has no sample in 10 ms")
            await self.push_error(f"cannot write audio: {error}", error, force_treat_as_permanent=True)
            return
        if await _open_room(self, self._room):
            await self.set_transport_ready(frame)

    async def set_transport_ready(self, frame: StartFrame):
        # pipecat's, with the output's own media sender; the transport refuses output destinations,
        # so the default one is all it starts.
        self._media_senders[None] = sender = _MediaSender(
            self,
            destination=None,
            sample_rate=self.sample_rate,
            audio_chunk_size=self.audio_chunk_size,
            params=self._params,
        )
        await sender.start(frame)
        await self.push_frame(OutputTransportReadyFrame(), FrameDirection.UPSTREAM)

    async def _end_stopped_senders(self) -> None:
        """Ends the media senders a `StopFrame` left running: pipecat keeps a stopped processor
        running, and `set_transport_ready` starts new senders in their place at the next
        `StartFrame`. The audio queued before the `StopFrame` was written as it went through."""
        senders = list(self._media_senders.values())
        self._media_senders.clear()
        for sender in senders:
            await sender.cleanup()

    async def stop(self, frame: EndFrame):
        # pipecat writes the audio queued before the EndFrame, in chunks of `audio_out_10ms_chunks`;
        # the speech's last chunk, short of one, is queued ahead of it, padded with silence, as
        # pipecat does at a TTSStoppedFrame. The audio task writes the speech's last packet after
        # it.
        try:
            try:
                if self._params.audio_out_enabled:
                    for sender in self._media_senders.values():
                        await sender._enqueue_flushed_audio_buffer()
            finally:
                await super().stop(frame)
        except Exception as error:
            await self._report_write_failure(f"cannot write the end of the speech: {error}", error)
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

    async def process_frame(self, frame: Frame, direction: FrameDirection):
        # The output publishes one broadcast, pipecat's default destination: a frame for another
        # destination, as for another transport's output in the same pipeline, goes on as it came,
        # leaving the output's resampler, senders and room alone. It goes on before pipecat's
        # processor handles a CancelFrame or an InterruptionFrame, and past the end of the speech
        # `push_frame` writes at a TTSStoppedFrame or a StopFrame, which are the pipeline's only
        # without a destination.
        if frame.transport_destination is not None:
            await super().push_frame(frame, direction)
            return
        audio_in = self._audio_in if self._params.audio_out_enabled else None
        # Before pipecat's processor and media sender handle the interruption, which waits for an
        # uninterruptible picture being written: the chunk being written stops at its next packet,
        # and the interruptible chunks queued after it are dropped at once, so none is written
        # meanwhile.
        if isinstance(frame, InterruptionFrame):
            self._room.interrupt_audio()
            sender = self._media_senders.get(None)
            if isinstance(sender, _MediaSender):
                sender.drop_interruptible_audio()
            if audio_in is not None:
                audio_in.interrupt()
        # pipecat's media sender cuts the audio of consecutive frames into chunks, each labelled
        # with the output's sample rate and the channel count of the frame that completed it, and
        # its own resampler takes one input rate and mono only: each frame is queued at the output's
        # rate and channel count, and one that cannot be is dropped here.
        elif direction == FrameDirection.DOWNSTREAM and audio_in is not None and isinstance(frame, OutputAudioRawFrame):
            try:
                frame = audio_in.normalized(frame)
            except ValueError as error:
                await FrameProcessor.process_frame(self, frame, direction)
                await self.push_error(f"cannot write audio: {error}", error)
                return
        # The end of the speech, which makes pipecat write its last chunk. pipecat queues that
        # chunk, short of `audio_out_10ms_chunks`, only at a TTSStoppedFrame, and the output's
        # `stop` does at an EndFrame; the senders a StopFrame leaves are replaced at the next
        # StartFrame, which would drop it, and the pipeline flush probe of
        # `PipelineWorker.flush_pipeline` tells that all the frames before it were handled, which
        # that chunk would belie.
        elif (
            direction == FrameDirection.DOWNSTREAM
            and audio_in is not None
            and isinstance(frame, (TTSStoppedFrame, EndFrame, StopFrame, PipelineFlushFrame))
        ):
            await self._queue_end_of_speech(
                audio_in, direction, last_chunk=isinstance(frame, (StopFrame, PipelineFlushFrame))
            )
        # pipecat's video sender cycles a sprite's pictures and resizes each picture before
        # `write_video_frame`, and an empty sprite or a picture it cannot resize would end its task
        # for good: each picture is queued as the encoder reads it, and a frame the encoder would
        # refuse or pipecat could not show is dropped here.
        elif (
            direction == FrameDirection.DOWNSTREAM
            and self._params.video_out_enabled
            and isinstance(frame, (OutputImageRawFrame, SpriteFrame))
        ):
            try:
                frame = self._normalized_pictures(frame)
            except ValueError as error:
                await FrameProcessor.process_frame(self, frame, direction)
                await self.push_error(f"cannot write a picture: {error}", error)
                return
            # pipecat shows a sprite, and a picture synchronized with the audio once the audio
            # queued before it was written, by making them the pictures its video sender cycles,
            # which a live sender does not read: a sprite's pictures go to the live queue one after
            # the other, each as interruptible as the sprite, an assistant image among them too,
            # which stays out of the assistant's context, as pipecat passes on no picture of a
            # sprite. A synchronized picture goes behind the chunks queued as a `_SyncedPicture`
            # noting the speech before it that the resampler and pipecat's last chunk hold, which is
            # queued only after it; once pipecat's sender reached it, `write_transport_frame` has it
            # written at the timestamp where that speech ends.
            sender = self._media_senders.get(frame.transport_destination)
            if isinstance(sender, _MediaSender) and isinstance(frame, OutputImageRawFrame) and frame.sync_with_audio:
                await FrameProcessor.process_frame(self, frame, direction)
                pending = audio_in.held_micros() + sender.held_audio_micros() if audio_in is not None else 0.0
                await sender.handle_synced_picture(_SyncedPicture.of(frame, pending))
                return
            if self._params.video_out_is_live and isinstance(sender, _MediaSender) and isinstance(frame, SpriteFrame):
                await FrameProcessor.process_frame(self, frame, direction)
                pictures = [
                    _copied(picture, sync_with_audio=False, interruptible=frame.interruptible) for picture in frame.images
                ]
                await sender.queue_live_pictures(pictures)
                return
        await super().process_frame(frame, direction)

    async def _queue_end_of_speech(self, audio_in: "_AudioNormalizer", direction: FrameDirection, *, last_chunk: bool):
        """Queues what the resampler holds of the speech ahead of the frame being processed and,
        with `last_chunk`, pipecat's last chunk of it, short of `audio_out_10ms_chunks`, padded with
        silence. A failure is pushed as an ErrorFrame, and the frame goes on all the same, so an
        EndFrame still stops the output and ends the pipeline."""
        try:
            tail = audio_in.flush()
            if tail is not None:
                await super().process_frame(tail, direction)
            if last_chunk:
                for sender in self._media_senders.values():
                    await sender._enqueue_flushed_audio_buffer()
        except Exception as error:
            await self.push_error(f"cannot write the end of the speech: {error}", error)

    async def _finish_speech(self) -> None:
        """Writes the speech's last packet, which the encoder holds; a failure is pushed upstream as
        a failed audio write is."""
        try:
            self._room.finish_audio()
        except Exception as error:
            await self._report_write_failure(f"cannot write the end of the speech: {error}", error)

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

    def _normalized_pictures(self, frame: OutputImageRawFrame | SpriteFrame) -> OutputImageRawFrame | SpriteFrame:
        """A copy of `frame` whose pictures carry their image as bytes, their color format, RGB for
        one without, and their size as two integers. Raises ValueError for a sprite without
        pictures, and for a picture the encoder would refuse or pipecat's video sender could not
        resize to the output's size."""
        if isinstance(frame, OutputImageRawFrame):
            return self._normalized_picture(frame)
        if not frame.images:
            raise ValueError("a sprite without pictures")
        return _copied(frame, images=[self._normalized_picture(picture) for picture in frame.images])

    def _normalized_picture(self, picture: OutputImageRawFrame) -> OutputImageRawFrame:
        color_format = picture.format or "RGB"
        image, size = _checked_picture(picture.image, picture.size, color_format)
        output_size = (self._params.video_out_width, self._params.video_out_height)
        # The media sender resizes with Pillow, as pipecat's does, which reads no BGR.
        if color_format in ("BGR", "BGRA") and size != output_size:
            raise ValueError(f"pipecat cannot resize a {color_format} picture to {output_size[0]}x{output_size[1]}")
        return _copied(picture, image=image, format=color_format, size=size)

    async def write_transport_frame(self, frame: Frame):
        # pipecat's audio task reached a synchronized picture: the chunks queued before it were
        # written, so the speech before it ends after the packets written, what the Opus encoder
        # holds and what the output held short of a chunk as the picture came. It is written from
        # now, at that timestamp, so it neither waits for speech after it nor stalls the pictures
        # after it, and off the audio task, whose speech its encode would hold up. The audio task
        # waits for an assistant image, so it is passed on before the frames after it.
        sender = self._media_senders.get(frame.transport_destination)
        if isinstance(frame, _SyncedPicture) and isinstance(sender, _MediaSender):
            writing = sender.write_synced_picture(frame, self._room.speech_end_micros(frame.pending_micros))
            if isinstance(frame.picture, AssistantImageRawFrame):
                await asyncio.wait([writing])

    async def push_frame(self, frame: Frame, direction: FrameDirection = FrameDirection.DOWNSTREAM):
        # The audio task passes a picture it reached on, as pipecat does: the draw or the write of
        # a picture queued behind the audio passes the picture on instead, once written.
        if isinstance(frame, (_SyncedPicture, _AssistantImage)):
            return
        sender = self._media_senders.get(frame.transport_destination)
        # pipecat's media sender pushes a TTSStoppedFrame, a StopFrame or a pipeline flush probe on
        # once it wrote the speech queued ahead of it: the utterance's last packet is written then,
        # before an interruption could drop it. The senders a StopFrame leaves are replaced at the
        # next StartFrame, which would drop the pictures given to them and not yet written, and a
        # flush probe tells that the frames before it were handled: the pictures given before it,
        # as it found them on reaching the output, are written first, as at an EndFrame.
        downstream = direction == FrameDirection.DOWNSTREAM
        if isinstance(frame, (TTSStoppedFrame, StopFrame, PipelineFlushFrame)) and downstream:
            await self._finish_speech()
        if isinstance(frame, (StopFrame, PipelineFlushFrame)) and downstream and isinstance(sender, _MediaSender):
            await sender.write_given_pictures(frame)
        await super().push_frame(frame, direction)

    # pipecat's media sender writes the audio and the messages in one task and the pictures in
    # another, which a write that raises would end for good: a failed write is pushed upstream as an
    # ErrorFrame instead, and the frames after it are written. Once publishing the media source
    # failed, which it does for good, the room drops the audio and pictures after it, and the write
    # returns False, so pipecat does not pass the audio on as sent.

    async def write_audio_frame(self, frame: OutputAudioRawFrame) -> bool:
        try:
            return await self._room.write_audio(
                frame.audio, frame.sample_rate, frame.num_channels, interruptible=frame.interruptible
            )
        except Exception as error:
            await self._report_write_failure(f"cannot write audio: {error}", error)
            return False

    async def write_video_frame(self, frame: OutputImageRawFrame) -> bool:
        return await self._write_picture(frame, None)

    async def _write_picture(self, frame: OutputImageRawFrame, timestamp_micros: int | None) -> bool:
        try:
            return await self._room.write_picture(
                frame.image, frame.size, frame.format or "RGB", timestamp_micros=timestamp_micros
            )
        except Exception as error:
            await self._report_write_failure(f"cannot write a picture: {error}", error)
            return False

    async def _report_write_failure(self, message: str, error: Exception) -> None:
        """Pushes a failed media write upstream as an `ErrorFrame`, non-fatal for a failure such as
        a picture the encoder refuses. The one that found publishing the media source failed is
        treated as permanent when the output writes no data track or event timeline: media was all
        it could do."""
        await self.push_error(message, error, force_treat_as_permanent=self._media_only and self._room.media_failed)

    async def send_message(self, frame: OutputTransportMessageFrame | OutputTransportMessageUrgentFrame):
        # pipecat passes a non-urgent message on by itself; an urgent one this transport does not write
        # goes on to the next processor, such as another transport's output.
        written = await self._room.write_message(frame.message)
        if not written and isinstance(frame, OutputTransportMessageUrgentFrame):
            await self.push_frame(frame)


class _MediaSender(BaseOutputTransport.MediaSender):
    """pipecat's media sender of `MoqWasmOutputTransport`."""

    def __init__(self, transport: "MoqWasmOutputTransport", **kwargs):
        super().__init__(transport, **kwargs)
        self._output = transport
        # The pictures a sender that is not live was given last to show, a picture or a sprite's;
        # None before it was given any.
        self._still: list[OutputImageRawFrame] | None = None
        # Whether the picture the output wrote last is one of the still's, so that a flush or the
        # end finds the still shown, not an assistant image drawn over it.
        self._still_written = False
        # How many pictures, sprites, assistant images and synchronized pictures the sender was
        # given, which numbers each in the order it was given, and the number of the still shown.
        self._given_stills = 0
        self._shown_still = 0
        # The synchronized pictures given and not yet written, oldest first: those pipecat's audio
        # task has not reached, and those it released, being written each after the one before it.
        self._synced: list[_SyncedPicture] = []
        self._synced_bound = _video_queue_bound(self._params)
        # Whether a synchronized picture was dropped since all those given were last written.
        self._synced_dropping = False
        # For each StopFrame or flush probe queued behind the audio, by id, the events of the live
        # pictures given before it, as `_given_live_pictures` gives them, and the number of
        # pictures given before it, past which a still is not the probe's to draw.
        self._live_before: dict[int, list[asyncio.Event]] = {}
        self._given_before: dict[int, int] = {}
        # Resolved once the interruption being handled created the video task again; None outside
        # one.
        self._interrupting: asyncio.Future[None] | None = None

    def _clear_audio_buffer(self):
        # pipecat clears the audio short of a chunk as a sender starts, holding none, and as the bot
        # stops speaking, which a pause in the speech makes it do: the audio is kept for the speech
        # after the pause or the next flush, as at a TTSStoppedFrame, an EndFrame or a StopFrame.
        # An interruption drops the interruptible part of it itself (`handle_interruptions`).
        pass

    def _create_video_task(self):
        created = self._video_task is None
        # pipecat gives the task it creates again at an interruption a new queue: the live queue is
        # kept instead, so a picture queued while the interruption replaced the task is not lost.
        queue = getattr(self, "_video_queue", None)
        super()._create_video_task()
        # The live video queue tells which picture is being written; the task reads it only once it
        # runs, after this.
        if created and self._video_task is not None and self._params.video_out_is_live:
            if isinstance(queue, _LiveVideoQueue):
                queue.abandon_writing()
                self._video_queue = queue
            else:
                self._video_queue = _LiveVideoQueue(_video_queue_bound(self._params))

    async def handle_image_frame(self, frame: OutputImageRawFrame | SpriteFrame):
        # pipecat shows an assistant image here and queues it behind the audio as well
        # (`handle_sync_frame`), where its audio task passes it on to the assistant's context: the
        # image is drawn there only, so it is shown if and only if it is passed on, once, and an
        # interruption drops it from both.
        if isinstance(frame, AssistantImageRawFrame):
            return
        self._shown_still = self._next_still()
        await super().handle_image_frame(frame)
        await self._report_dropping()

    async def queue_live_pictures(self, pictures: list[OutputImageRawFrame]) -> None:
        """Queues `pictures` on a live sender's queue one after the other, as pipecat queues a live
        picture, an assistant image as any other picture."""
        for picture in pictures:
            await self._video_queue.put(picture)
        await self._report_dropping()

    async def _report_dropping(self) -> None:
        """Reports with a non-fatal `ErrorFrame` that the live queue began dropping pictures."""
        queue = getattr(self, "_video_queue", None)
        if isinstance(queue, _LiveVideoQueue) and queue.began_dropping():
            error = RuntimeError(f"more than {queue.bound} pictures queued")
            await self._transport.push_error(
                f"dropping the oldest pictures: they come faster than {self._params.video_out_framerate} a second",
                error,
            )

    def drop_interruptible_audio(self) -> None:
        """Drops the interruptible audio queued, as pipecat's `handle_interruptions` does, and the
        interruptible synchronized pictures queued with it; those being written are written."""
        self._audio_queue.reset()
        self._synced = [synced for synced in self._synced if synced.writing is not None or not synced.interruptible]

    async def handle_synced_picture(self, synced: "_SyncedPicture") -> None:
        """Queues `synced` behind the audio queued, to be written once pipecat's audio task reached
        it. Past `LIVE_VIDEO_QUEUE_SECONDS` of synchronized pictures not yet written, as when their
        encodes are slower than the framerate or speech far ahead of the clock carries them, it is
        dropped instead, reported once with a non-fatal `ErrorFrame` until all were written."""
        if not self._synced:
            self._synced_dropping = False
        if len(self._synced) >= self._synced_bound:
            if not self._synced_dropping:
                self._synced_dropping = True
                error = RuntimeError(f"more than {self._synced_bound} synchronized pictures wait to be written")
                await self._transport.push_error(f"dropping the synchronized pictures past them: {error}", error)
            return
        synced.sequence = self._next_still()
        self._synced.append(synced)
        await self.handle_sync_frame(synced)

    def write_synced_picture(self, synced: "_SyncedPicture", timestamp_micros: int | None) -> asyncio.Task:
        """Writes `synced`, which pipecat's audio task reached, at `timestamp_micros`, in a task of
        its own once the synchronized picture released before it was written; returns that task."""
        synced.writing = asyncio.create_task(self._write_synced(synced, timestamp_micros, self._released_write()))
        return synced.writing

    def _released_write(self) -> asyncio.Task | None:
        """The write of the synchronized picture pipecat's audio task released last, while it
        lasts."""
        return next((other.writing for other in reversed(self._synced) if other.writing is not None), None)

    async def _write_synced(
        self, synced: "_SyncedPicture", timestamp_micros: int | None, previous: asyncio.Task | None
    ) -> None:
        # An output that is not live then shows the picture as its still, as pipecat does, and the
        # picture goes on downstream, as pipecat's audio task passes it on.
        try:
            if previous is not None:
                await asyncio.wait([previous])
            written = await self._output._write_picture(synced.picture, timestamp_micros)
        finally:
            self._synced = [other for other in self._synced if other is not synced]
        if written:
            self._still_written = False
            await self._show_still(synced.picture, synced.sequence)
            await self._transport.push_frame(synced.picture)

    async def cleanup(self):
        writes = [synced.writing for synced in self._synced if synced.writing is not None]
        self._synced.clear()
        try:
            for write in writes:
                write.cancel()
            await asyncio.gather(*writes, return_exceptions=True)
        finally:
            await super().cleanup()

    async def handle_interruptions(self, _: InterruptionFrame):
        # pipecat cancels the picture being written and creates the video task again, on the same
        # live queue (`_create_video_task`): the interruptible pictures queued are dropped, and an
        # uninterruptible one being written is written first.
        interrupting = self._interrupting = asyncio.get_running_loop().create_future()
        try:
            queue = self._video_queue if self._video_task is not None else None
            if isinstance(queue, _LiveVideoQueue):
                queue.drop_interruptible()
                await queue.written(uninterruptible_only=True, task=self._video_task)
            await super().handle_interruptions(_)
        finally:
            interrupting.set_result(None)
            if self._interrupting is interrupting:
                self._interrupting = None

    async def _set_video_image(self, image: OutputImageRawFrame):
        await super()._set_video_image(image)
        self._still, self._still_written = [image], False

    async def _set_video_images(self, images: list[OutputImageRawFrame]):
        await super()._set_video_images(images)
        self._still, self._still_written = images, False

    def _next_still(self) -> int:
        """The number of the picture given now, in the order of those the sender was given."""
        self._given_stills += 1
        return self._given_stills

    async def _show_still(self, image: OutputImageRawFrame, sequence: int):
        """Makes `image`, the `sequence`th picture given, which the output wrote already, the still
        a sender that is not live shows from its next frame on; an assistant image only when the
        still shown was given before it, as pipecat showed it as it was given."""
        if self._params.video_out_is_live:
            return
        if isinstance(image, AssistantImageRawFrame) and sequence < self._shown_still:
            return
        self._shown_still = sequence
        await self._set_video_image(image)
        self._still_written = True

    async def _draw_image(self, frame: OutputImageRawFrame):
        await self._draw(frame)

    async def _handle_frame(self, frame: Frame):
        # pipecat's audio task reached an assistant image, the audio queued before it written.
        if isinstance(frame, _AssistantImage):
            await self._draw_assistant_image(frame)
            return
        await super()._handle_frame(frame)

    async def _draw_assistant_image(self, queued: "_AssistantImage") -> None:
        """Writes the image `queued`, after the synchronized picture released before it, outside the
        live queue, and passes it on once written; a sender that is not live then shows it as its
        still, unless it shows one given after it already. A sender without video passes it on."""
        image = queued.picture
        if self._params.video_out_enabled:
            previous = self._released_write()
            if previous is not None:
                await asyncio.wait([previous])
            if not await self._draw(image):
                return
            await self._show_still(image, queued.sequence)
        await self._transport.push_frame(image)

    async def _draw(self, frame: OutputImageRawFrame) -> bool:
        """Writes `frame` as pipecat's `_draw_image` does, resized to the output's size in the
        sender's executor, and returns whether the output wrote it, which pipecat's does not tell. A
        resize that fails is pushed upstream as a non-fatal `ErrorFrame`, as a failed write is, so
        the audio or video task drawing the frame goes on."""
        size = (self._params.video_out_width, self._params.video_out_height)
        resized = frame
        if frame.size != size:
            try:
                resized = await self._transport.get_event_loop().run_in_executor(self._executor, _resized, frame, size)
            except Exception as error:
                await self._transport.push_error(f"cannot write a picture: {error}", error)
                return False
        written = await self._output.write_video_frame(resized)
        # A still the output failed to write counts as written, so that it is not tried again at
        # each flush; any other picture written leaves the still to be written again.
        if self._still is not None and any(frame is picture for picture in self._still):
            self._still_written = True
        elif written:
            self._still_written = False
        return written

    def held_audio_micros(self) -> float:
        """Microseconds of audio held short of a chunk, which is queued once the audio after it
        completes the chunk."""
        return self._buffered_audio_bytes * 1_000_000 / (2 * self._params.audio_out_channels * self._sample_rate)

    async def _audio_task_handler(self):
        await super()._audio_task_handler()
        # The speech's last packet, which the encoder holds, before the pictures are written.
        await self._output._finish_speech()
        # The audio task returns only at an EndFrame, after which pipecat cancels the video task.
        await self.write_given_pictures()

    async def handle_sync_frame(self, frame: Frame):
        # A StopFrame or a flush probe stands for the frames before it: the still it finds is drawn
        # now, unless it was written last, before a picture given after the probe replaces it, and
        # the live pictures it finds queued, and the still, are those the output writes before
        # passing it on.
        if isinstance(frame, (StopFrame, PipelineFlushFrame)):
            await self._write_given_still()
            self._live_before[frame.id] = self._given_live_pictures()
            self._given_before[frame.id] = self._given_stills
        # An assistant image goes behind the audio numbered, and leaves the sender once drawn or
        # dropped.
        elif isinstance(frame, AssistantImageRawFrame):
            frame = _AssistantImage.of(frame, self._next_still())
        await super().handle_sync_frame(frame)

    async def write_given_pictures(self, frame: Frame | None = None) -> None:
        """Writes the pictures the sender was given before `frame`, a StopFrame or a flush probe,
        or else, at the end, all it was given, and has not written: the synchronized pictures
        pipecat's audio task released, within `LIVE_VIDEO_DRAIN_TIMEOUT_SECONDS`, then those of a
        live sender's queue, within as long, even once an interruption created its video task
        again, and the still a sender that is not live was given last, which its video task writes
        only at its next frame, drawn at once, within `STILL_WRITE_TIMEOUT_SECONDS`, unless it is
        the picture the output wrote last or was given after `frame`: an assistant image the audio
        task drew over it since `handle_sync_frame` drew the still `frame` found leaves it to be
        drawn again, and a still given after `frame` is left to the video task. A deadline that
        expires is reported as a non-fatal `ErrorFrame`."""
        writes = [synced.writing for synced in self._synced if synced.writing is not None]
        if writes:
            _, unwritten = await asyncio.wait(writes, timeout=LIVE_VIDEO_DRAIN_TIMEOUT_SECONDS)
            if unwritten:
                await self._report_unwritten(
                    f"the synchronized pictures were not written in {LIVE_VIDEO_DRAIN_TIMEOUT_SECONDS} s"
                )
        given_before = self._given_before.pop(frame.id, None) if frame is not None else None
        await self._write_given_still(given_before)
        live = self._live_before.pop(frame.id, None) if frame is not None else None
        if live is None:
            live = self._given_live_pictures()
        if not live:
            return
        loop = asyncio.get_running_loop()
        deadline = loop.time() + LIVE_VIDEO_DRAIN_TIMEOUT_SECONDS
        written = asyncio.ensure_future(asyncio.gather(*(picture.wait() for picture in live)))
        try:
            while True:
                # An interruption ends the video task and creates it again, and the new one writes
                # the uninterruptible pictures queued: the wait goes on until a video task ends for
                # good.
                ending = self._interrupting or self._video_task
                if ending is None:
                    return
                done, _ = await asyncio.wait(
                    [written, ending], timeout=deadline - loop.time(), return_when=asyncio.FIRST_COMPLETED
                )
                if not done:
                    break
                if written in done or (ending is self._video_task and self._interrupting is None):
                    return
        finally:
            written.cancel()
            # A cancelled gather ends with a CancelledError as its exception, which the loop reports
            # as never retrieved unless it is.
            written.add_done_callback(lambda gathered: gathered.cancelled() or gathered.exception())
        await self._report_unwritten(f"the pictures queued were not written in {LIVE_VIDEO_DRAIN_TIMEOUT_SECONDS} s")

    def _given_live_pictures(self) -> list[asyncio.Event]:
        """The events of the pictures a live sender's queue holds and is writing, set once each was
        written or dropped."""
        queue = getattr(self, "_video_queue", None)
        return queue.unfinished() if isinstance(queue, _LiveVideoQueue) else []

    async def _write_given_still(self, given_before: int | None = None) -> None:
        """Draws the still a sender that is not live was given last at once, unless it is the
        picture the output wrote last or one of those given past the first `given_before`, within
        `STILL_WRITE_TIMEOUT_SECONDS`."""
        still = self._still
        if self._video_task is None or still is None or self._still_written:
            return
        if given_before is not None and self._shown_still > given_before:
            return
        try:
            await asyncio.wait_for(self._draw_image(still[0]), STILL_WRITE_TIMEOUT_SECONDS)
        except TimeoutError:
            await self._report_unwritten(f"the last still was not written in {STILL_WRITE_TIMEOUT_SECONDS} s")

    async def _report_unwritten(self, message: str) -> None:
        await self._transport.push_error(f"cannot write the pictures given: {message}", TimeoutError(message))


def _video_queue_bound(params: TransportParams) -> int:
    """The pictures `LIVE_VIDEO_QUEUE_SECONDS` hold at the output's framerate."""
    return max(1, round(LIVE_VIDEO_QUEUE_SECONDS * params.video_out_framerate))


@dataclass(eq=False)
class _QueuedPicture:
    """A picture of a live video queue, with the event set once it was written or dropped."""

    picture: OutputImageRawFrame
    done: asyncio.Event = field(default_factory=asyncio.Event)


class _LiveVideoQueue(asyncio.Queue):
    """The queue of a live video sender, which follows each picture put until it was written or
    dropped, and knows the picture its task took and has not yet marked done, the one being
    written. It holds `bound` pictures at most: a picture put past them drops the oldest
    interruptible one queued, or the oldest one when all are uninterruptible."""

    def __init__(self, bound: int):
        super().__init__()
        self.bound = bound
        self._writing: _QueuedPicture | None = None
        # The pictures put and neither written nor dropped, in the order they were put.
        self._unfinished: list[_QueuedPicture] = []
        # Whether a picture was dropped since the queue was last empty, and whether that was
        # reported by `began_dropping`.
        self._dropping = False
        self._dropping_reported = False

    def put_nowait(self, item: OutputImageRawFrame) -> None:
        if self.qsize() >= self.bound:
            queued = self._take_queued()
            dropped = next((entry for entry in queued if entry.picture.interruptible), queued[0])
            self._finish(dropped)
            for entry in queued:
                if entry is not dropped:
                    super().put_nowait(entry)
            self._dropping = True
        entry = _QueuedPicture(item)
        self._unfinished.append(entry)
        super().put_nowait(entry)

    def unfinished(self) -> list[asyncio.Event]:
        """The events of the pictures queued and being written, set once each was written or
        dropped."""
        return [entry.done for entry in self._unfinished]

    def _finish(self, entry: _QueuedPicture) -> None:
        entry.done.set()
        self._unfinished.remove(entry)

    def began_dropping(self) -> bool:
        """Whether the queue began dropping pictures since it was last empty; True once for each
        time it did."""
        if not self._dropping or self._dropping_reported:
            return False
        self._dropping_reported = True
        return True

    async def get(self) -> OutputImageRawFrame:
        entry = await super().get()
        self._writing = entry
        if self.empty():
            self._dropping = self._dropping_reported = False
        return entry.picture

    def task_done(self) -> None:
        super().task_done()
        writing, self._writing = self._writing, None
        if writing is not None:
            self._finish(writing)

    def drop_interruptible(self) -> None:
        """Drops the interruptible pictures queued, leaving the one being written."""
        for entry in self._take_queued():
            if entry.picture.interruptible:
                self._finish(entry)
            else:
                super().put_nowait(entry)

    def abandon_writing(self) -> None:
        """Marks the picture being written done, as one whose task was cancelled, which never
        marks it: an interruptible one is dropped, an uninterruptible one queued again, first."""
        writing, self._writing = self._writing, None
        if writing is None:
            return
        super().task_done()
        if writing.picture.interruptible:
            self._finish(writing)
            return
        for entry in [writing, *self._take_queued()]:
            super().put_nowait(entry)

    def _take_queued(self) -> list[_QueuedPicture]:
        """Takes out the pictures queued, leaving the one being written."""
        taken = []
        while not self.empty():
            taken.append(self.get_nowait())
            super().task_done()
        return taken

    async def written(self, *, uninterruptible_only: bool, task: asyncio.Task | None) -> None:
        """Returns once the picture being written, uninterruptible with `uninterruptible_only`, was
        written, or `task` ended."""
        writing = self._writing
        if writing is None or (uninterruptible_only and writing.picture.interruptible) or task is None:
            return
        done = asyncio.ensure_future(writing.done.wait())
        try:
            await asyncio.wait([done, task], return_when=asyncio.FIRST_COMPLETED)
        finally:
            done.cancel()


@dataclass
class _SyncedPicture(DataFrame):
    """A picture synchronized with the audio, queued behind the audio before it, with the
    microseconds of that audio the output held short of the audio queue as it came,
    `pending_micros`; `writing` once pipecat's audio task released it."""

    picture: OutputImageRawFrame
    pending_micros: float
    writing: asyncio.Task | None = None
    # The number of the picture in the order its sender was given pictures to show.
    sequence: int = 0

    @classmethod
    def of(cls, picture: OutputImageRawFrame, pending_micros: float) -> "_SyncedPicture":
        synced = cls(picture=picture, pending_micros=pending_micros)
        synced.interruptible = picture.interruptible
        synced.transport_destination = picture.transport_destination
        return synced


@dataclass
class _AssistantImage(DataFrame):
    """An assistant image queued behind the audio before it, the `sequence`th picture its sender was
    given to show."""

    picture: AssistantImageRawFrame
    sequence: int

    @classmethod
    def of(cls, picture: AssistantImageRawFrame, sequence: int) -> "_AssistantImage":
        queued = cls(picture=picture, sequence=sequence)
        queued.interruptible = picture.interruptible
        queued.transport_destination = picture.transport_destination
        return queued


class _AudioNormalizer:
    """Brings the audio of each `OutputAudioRawFrame` to the output's sample rate and channel
    count, resampled as one stream. The resampler holds back a few samples of each frame: `flush`
    returns them at the end of the speech, as a frame like the last one, and `interrupt` drops what
    it holds of interruptible speech."""

    def __init__(self, sample_rate: int, num_channels: int):
        self._sample_rate = sample_rate
        self._num_channels = num_channels
        self._resampler = _PcmResampler(sample_rate, num_channels)
        # The last frame resampled, whose class and flags what the resampler holds takes; None while
        # it holds nothing.
        self._last: OutputAudioRawFrame | None = None

    def normalized(self, frame: OutputAudioRawFrame) -> OutputAudioRawFrame:
        """A copy of `frame` at the output's sample rate and channel count. Raises ValueError for
        audio that is not 16-bit mono or stereo PCM."""
        audio = self._resampler.resample(frame.audio, frame.sample_rate, frame.num_channels)
        self._last = frame
        return self._frame(frame, audio)

    def flush(self) -> OutputAudioRawFrame | None:
        """What the resampler holds of the speech, as a frame like its last one; None when it holds
        nothing. Raises what failed the resampler, which then holds nothing."""
        last, self._last = self._last, None
        if last is None:
            return None
        try:
            audio = self._resampler.flush()
        except BaseException:
            self._resampler.reset()
            raise
        return self._frame(last, audio) if audio else None

    def held_micros(self) -> float:
        """Microseconds of the speech the resampler holds back."""
        return self._resampler.held_samples * 1_000_000 / self._sample_rate

    def interrupt(self) -> None:
        if self._last is not None and self._last.interruptible:
            self._resampler.reset()
            self._last = None

    def _frame(self, frame: OutputAudioRawFrame, audio: bytes) -> OutputAudioRawFrame:
        return _copied(
            frame,
            audio=audio,
            sample_rate=self._sample_rate,
            num_channels=self._num_channels,
            num_frames=len(audio) // (self._num_channels * 2),
        )


def _resized(frame: OutputImageRawFrame, size: tuple[int, int]) -> OutputImageRawFrame:
    """`frame` resized to `size` with Pillow, as pipecat's media sender resizes it."""
    image = Image.frombytes(frame.format or "RGB", frame.size, frame.image).resize(size)
    return OutputImageRawFrame(image.tobytes(), image.size, image.mode)


def _copied(frame: FrameT, **changes: object) -> FrameT:
    """A copy of `frame`, of its class and with its id and flags, such as `interruptible`, with
    `changes` made to it, so the frame an upstream processor still holds is left as it was."""
    copied = copy.copy(frame)
    for name, value in changes.items():
        setattr(copied, name, value)
    return copied


def _has_soundfile_mixer(mixer: object) -> bool:
    """Whether `mixer` is, or maps a destination to, pipecat's `SoundfileMixer`, which mixes one
    sample of its mono file into each sample of the audio, so plays it twice as fast into stereo."""
    if mixer is None:
        return False
    try:
        from pipecat.audio.mixers.soundfile_mixer import SoundfileMixer
    except ImportError:  # Without `soundfile`, pipecat has no `SoundfileMixer`.
        return False
    mixers = mixer.values() if isinstance(mixer, Mapping) else [mixer]
    return any(isinstance(each, SoundfileMixer) for each in mixers)


class MoqWasmTransport(BaseTransport):
    """Takes the place of pipecat's `MOQTransport` on a `moq_wasm.Session`, which owns the connection
    and tokens. Every broadcast one level under `params.room` is a participant. It keeps a copy of
    `params` and of its lists and mappings, as `room` and an `audio_out_mixer` by destination:
    assigning its fields or changing those once it is built changes nothing.

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
    The output does the same once publishing its media failed when it writes no data track or event
    timeline, and when its audio sample rate, under 100 Hz, leaves no sample in pipecat's 10 ms
    chunks. A nonpositive `audio_out_10ms_chunks` raises ValueError, as does an `audio_out_mixer`
    that is or holds pipecat's `SoundfileMixer` with `audio_out_channels=2`: it mixes its mono files
    into mono audio only. An `audio_in_filter` raises ValueError too: the input carries every
    participant's audio, and one filter, such as pipecat's `RNNoiseFilter`, would carry one
    speaker's state into another's audio; run a `MoqWasmParticipantInput` per participant, each
    with a filter of its own.
    A room that cannot be left at an `EndFrame` or a `CancelFrame` is reported
    with a non-fatal `ErrorFrame`, and the frame goes on, so the pipeline ends.

    A processor serves one pipeline run: once pipecat cleaned up the input or the output at the end
    of its pipeline, `input()` or `output()` returns a new one, which opens the room again, so
    build each pipeline from them rather than from the processors of an earlier one. A `StopFrame`
    does not end the run: pipecat keeps the stopped processors running, in the room, and the next
    pipeline's `StartFrame` resumes them, the output's media senders replaced by new ones; the
    output writes all the speech queued before the `StopFrame`, its last chunk and packet padded
    with silence, as at an `EndFrame`. The probe of `PipelineWorker.flush_pipeline` passes the
    output only once it wrote the speech and the pictures given before it the same way: the still
    and the live pictures it found on reaching the output, not those given after it."""

    def __init__(
        self,
        session: Session,
        params: MoqWasmParams,
        input_name: str | None = None,
        output_name: str | None = None,
    ):
        super().__init__(input_name=input_name, output_name=output_name)
        params = _snapshot(params)
        if params.audio_out_destinations or params.video_out_destinations:
            raise ValueError("MoqWasmTransport publishes one broadcast: it takes no output destinations")
        if params.audio_in_filter is not None:
            raise ValueError(
                "MoqWasmTransport's input carries every participant's audio through one filter, which would mix"
                " their state: give each participant a MoqWasmParticipantInput with an audio_in_filter of its own"
            )
        if params.audio_out_channels not in (1, 2):
            raise ValueError("MoqWasmTransport takes mono or stereo output audio")
        if params.audio_out_channels != 1 and _has_soundfile_mixer(params.audio_out_mixer):
            raise ValueError(
                "pipecat's SoundfileMixer mixes its mono files into mono output audio only: set audio_out_channels=1"
            )
        # pipecat's media sender cuts the audio into chunks of this many 10 ms: with none, its loop
        # would queue empty chunks forever.
        if params.audio_out_10ms_chunks <= 0:
            raise ValueError("MoqWasmTransport takes a positive audio_out_10ms_chunks")
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
            audio_out_bitrate=params.audio_out_bitrate,
            audio_out_channels=params.audio_out_channels,
            video_out_bitrate=params.video_out_bitrate,
            data_out_tracks=params.data_out_tracks,
            event_timelines_out=params.event_timelines_out,
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
        every picture when it is None, or only for image requests with `on_request_only`. As in
        pipecat, a `framerate` of 0 means `on_request_only`, and is deprecated."""
        if framerate == 0 and not on_request_only:
            warn_deprecated(
                "`MoqWasmTransport.capture_participant_video(framerate=0)` is deprecated since 1.13.0 and "
                "will be removed in 2.0.0. Use `on_request_only=True` instead.",
                stacklevel=2,
            )
        # The room takes None for request-only pictures and infinity for every picture.
        if on_request_only or framerate == 0:
            rate = None
        else:
            rate = float("inf") if framerate is None else framerate
        await self._room.capture(participant_id, video_source, rate)

    async def release_participant_video(self, participant_id: str, video_source: str = CAMERA_SOURCE):
        """Stops receiving a video source of a participant, for a publisher that serves one profile to
        all its subscribers, such as onvif-ingest; the image requests waiting on it get an error.
        Returns once the source's track is unsubscribed, also when called again after a cancelled
        release. Not part of pipecat's API."""
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

    async def push_picture(self, picture: ReceivedPicture) -> bool:
        return await self._receiving_input().push_picture(picture)

    async def push_message(self, message: MoqWasmTrackMessage) -> None:
        await self._receiving_input().push_message(message)

    async def answer_image_requests(self, requests: list[UserImageRequestFrame], error: str) -> None:
        await self._receiving_input().answer_image_requests(requests, error)

    async def event(self, name: str, *args: object) -> None:
        await self._call_event_handler(name, *args)
