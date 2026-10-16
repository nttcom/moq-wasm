import enum
import math
from dataclasses import dataclass

from moq_ptz_tracking.ptz import Heading, PtzCalibration, PtzMove
from moq_ptz_tracking.vision import Answer, Position

# After losing the target the camera first turns the way the target was heading for this long.
PREDICT_SECONDS = 3.0
# A target that moved less than this share of the picture between two detections is taken to have
# left the picture on the side it was last seen on.
MIN_MOTION = 0.03
# Heading components below this share of the larger one are dropped, so a mostly sideways exit
# sweeps sideways only.
MIN_HEADING_SHARE = 0.3
# The camera reports no reliable position (a Tapo's GetStatus is stale), so the search dead-reckons
# from (0, 0) and turns back after half of a 360 degree turn: ~6.5 s at 0.27 of a ~100 degree view
# per second.
HALF_TURN_SECONDS = 6.5
# A step toward the target that shifts it by less than this share of the calibrated shift means the
# camera has reached its pan limit; STUCK_STEPS such steps in a row send the camera the other way round.
STUCK_SHIFT_RATIO = 0.3
STUCK_STEPS = 2
# The camera turns at most 360 degrees, so a target past a pan limit is reached the other way round;
# give up after about one turn.
AROUND_SECONDS = 14.0


@dataclass(frozen=True)
class Sweeping:
    heading: Heading


@dataclass(frozen=True)
class Halt:
    """The target is in the picture."""


@dataclass(frozen=True)
class Home:
    """Return to (0, 0) before searching sideways."""


Decision = PtzMove | Sweeping | Halt | Home


class Phase(enum.Enum):
    TRACKING = enum.auto()
    PREDICTING = enum.auto()
    SEARCHING = enum.auto()
    GOING_AROUND = enum.auto()


class PanPlanner:
    """Picks the camera's next action per located picture: step toward a visible target; for a lost
    one, sweep its last heading, then search sideways from (0, 0), turning back half a turn either side.
    A target held off center by a pan limit is reached the other way round (the camera turns at most
    360 degrees)."""

    def __init__(self, calibration: PtzCalibration):
        self._calibration = calibration
        self._phase = Phase.TRACKING
        self._previous: Position | None = None
        self._last_step = PtzMove(0.0, 0.0)
        self._motion: tuple[float, float] | None = None
        self._heading = Heading(1.0, 0.0)
        self._phase_started_at = 0.0
        self._lost_while_around = False
        self._stuck_steps = 0
        self._search_offset = 0.0
        self._swept_until: float | None = None

    def decide(self, answer: Position | Answer, now: float) -> Decision:
        if self._phase is Phase.GOING_AROUND:
            decision = self._go_around(answer, now)
            if decision is not None:
                return decision
        if isinstance(answer, Position):
            return self._track(answer, now)
        if self._phase is Phase.TRACKING:
            return self._predict(now)
        if self._phase is Phase.PREDICTING:
            if now - self._phase_started_at < PREDICT_SECONDS:
                return Sweeping(self._heading)
            return self._home(now, side_of(self._heading.pan))
        return self._search(now)

    def _track(self, answer: Position, now: float) -> Decision:
        if self._phase is not Phase.TRACKING:
            self._phase = Phase.TRACKING
            self._forget_target()
            return Halt()
        move = self._calibration.toward(answer)
        self._stuck_steps = self._stuck_steps + 1 if self._did_not_shift(answer.x, move) else 0
        if self._stuck_steps == STUCK_STEPS:
            self._forget_target()
            self._start(Phase.GOING_AROUND, now, Heading(-side_of(move.pan_seconds), 0.0))
            return Sweeping(self._heading)
        if self._previous is not None:
            self._motion = (
                answer.x - self._previous.x + self._last_step.pan_seconds * self._calibration.pan_per_second,
                answer.y - self._previous.y - self._last_step.tilt_seconds * self._calibration.tilt_per_second,
            )
        self._previous = answer
        self._last_step = move
        return move

    def _predict(self, now: float) -> Decision:
        heading = self._predicted_heading()
        if heading is None:
            return self._home(now, side_of(self._heading.pan))
        self._start(Phase.PREDICTING, now, heading)
        return Sweeping(heading)

    def _predicted_heading(self) -> Heading | None:
        if self._previous is None:
            return None
        if self._motion is not None and math.hypot(*self._motion) >= MIN_MOTION:
            dx, dy = self._motion
        else:
            dx, dy = self._previous.x - 0.5, self._previous.y - 0.5
        larger = max(abs(dx), abs(dy))
        if larger == 0:
            return None
        pan, tilt = dx / larger, -dy / larger
        return Heading(
            pan if abs(pan) >= MIN_HEADING_SHARE else 0.0,
            tilt if abs(tilt) >= MIN_HEADING_SHARE else 0.0,
        )

    def _home(self, now: float, direction: float) -> Home:
        self._start(Phase.SEARCHING, now, Heading(direction, 0.0))
        self._search_offset = 0.0
        self._swept_until = None
        return Home()

    def _search(self, now: float) -> Sweeping:
        """Dead reckoning starts at the first search decision after the camera is back at (0, 0)."""
        if self._swept_until is not None:
            self._search_offset += self._heading.pan * (now - self._swept_until)
        self._swept_until = now
        if self._search_offset * self._heading.pan >= HALF_TURN_SECONDS:
            self._heading = Heading(-self._heading.pan, 0.0)
        return Sweeping(self._heading)

    def _go_around(self, answer: Position | Answer, now: float) -> Decision | None:
        if answer is Answer.NOT_VISIBLE:
            self._lost_while_around = True
        elif self._lost_while_around:
            self._phase = Phase.TRACKING
            self._forget_target()
            return Halt()
        if now - self._phase_started_at < AROUND_SECONDS:
            return Sweeping(self._heading)
        return self._home(now, self._heading.pan)

    def _did_not_shift(self, x: float, move: PtzMove) -> bool:
        if self._previous is None or not move.pan_seconds or not self._last_step.pan_seconds:
            return False
        if side_of(self._last_step.pan_seconds) != side_of(move.pan_seconds):
            return False
        expected = abs(self._last_step.pan_seconds * self._calibration.pan_per_second)
        shifted = (self._previous.x - x) * side_of(self._last_step.pan_seconds)
        return shifted < STUCK_SHIFT_RATIO * expected

    def _forget_target(self) -> None:
        self._previous = None
        self._motion = None
        self._stuck_steps = 0

    def _start(self, phase: Phase, now: float, heading: Heading) -> None:
        self._phase = phase
        self._phase_started_at = now
        self._heading = heading
        self._lost_while_around = False


def side_of(pan: float) -> float:
    return math.copysign(1.0, pan) if pan else 1.0
