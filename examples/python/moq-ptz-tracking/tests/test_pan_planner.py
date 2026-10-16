from moq_ptz_tracking.pan_planner import (
    AROUND_SECONDS,
    PREDICT_SECONDS,
    HALF_TURN_SECONDS,
    Halt,
    Home,
    PanPlanner,
    Sweeping,
)
from moq_ptz_tracking.ptz import Heading, PtzCalibration, PtzMove
from moq_ptz_tracking.vision import Answer, Position

TAPO = PtzCalibration(pan_per_second=0.27, tilt_per_second=0.38, dead_zone=0.05)
AT_RIGHT_EDGE = Position(0.9, 0.5)
STEP_RIGHT = PtzMove(1.48, 0.0)
LOST = Answer.NOT_VISIBLE


def decide_all(planner: PanPlanner, answers, start: float = 0.0, interval: float = 1.0):
    return [planner.decide(answer, start + index * interval) for index, answer in enumerate(answers)]


def test_target_moving_up_and_right_is_followed_that_way_once_lost():
    # Arrange
    planner = PanPlanner(TAPO)
    decide_all(planner, [Position(0.5, 0.5), Position(0.5625, 0.4375)])

    # Act / Assert
    assert planner.decide(LOST, 2.0) == Sweeping(Heading(1.0, 1.0))


def test_target_that_stood_still_is_followed_toward_the_side_it_was_last_seen_on():
    # Arrange
    planner = PanPlanner(TAPO)
    decide_all(planner, [Position(0.2, 0.52), Position(0.2, 0.52)])

    # Act / Assert
    assert planner.decide(LOST, 2.0) == Sweeping(Heading(-1.0, 0.0))


def test_prediction_that_finds_nothing_levels_the_camera_then_searches_sideways():
    # Arrange
    planner = PanPlanner(TAPO)
    decide_all(planner, [Position(0.2, 0.5), Position(0.2, 0.5), LOST])

    # Act
    decisions = decide_all(planner, [LOST, LOST], start=2.0 + PREDICT_SECONDS)

    # Assert
    assert decisions == [Home(), Sweeping(Heading(-1.0, 0.0))]


def test_target_never_seen_levels_the_camera_at_once():
    # Act / Assert
    assert PanPlanner(TAPO).decide(LOST, 0.0) == Home()


def test_sideways_search_turns_back_half_a_turn_away_from_home():
    # Arrange
    planner = PanPlanner(TAPO)
    planner.decide(LOST, 0.0)

    # Act: the first sweep decision may come any time after Home
    decisions = [planner.decide(LOST, 10.0), planner.decide(LOST, 10.0 + HALF_TURN_SECONDS)]

    # Assert
    assert decisions == [Sweeping(Heading(1.0, 0.0)), Sweeping(Heading(-1.0, 0.0))]


def test_search_after_turning_back_reaches_the_other_side_after_a_whole_turn():
    # Arrange
    planner = PanPlanner(TAPO)
    decide_all(planner, [LOST, LOST, LOST], start=0.0, interval=HALF_TURN_SECONDS)

    # Act
    decisions = [planner.decide(LOST, 3 * HALF_TURN_SECONDS), planner.decide(LOST, 4 * HALF_TURN_SECONDS)]

    # Assert
    assert decisions == [Sweeping(Heading(-1.0, 0.0)), Sweeping(Heading(1.0, 0.0))]


def test_lost_target_seen_again_halts_the_sweep():
    # Arrange
    planner = PanPlanner(TAPO)
    planner.decide(LOST, 0.0)

    # Act / Assert
    assert planner.decide(Position(0.6, 0.5), 1.0) == Halt()


def test_target_kept_at_the_edge_by_a_pan_limit_is_reached_the_other_way_round():
    # Act
    decisions = decide_all(planner := PanPlanner(TAPO), [AT_RIGHT_EDGE] * 4)

    # Assert: two steps toward it did not shift it, so the camera turns left even while it is still in view
    assert decisions == [STEP_RIGHT, STEP_RIGHT, Sweeping(Heading(-1.0, 0.0)), Sweeping(Heading(-1.0, 0.0))]
    assert decide_all(planner, [LOST, Position(0.3, 0.5)], start=4.0) == [Sweeping(Heading(-1.0, 0.0)), Halt()]


def test_going_around_gives_up_after_about_one_turn_and_keeps_searching():
    # Arrange
    planner = PanPlanner(TAPO)
    decide_all(planner, [AT_RIGHT_EDGE] * 3)

    # Act
    decisions = decide_all(planner, [LOST, LOST], start=3.0, interval=AROUND_SECONDS)

    # Assert
    assert decisions == [Sweeping(Heading(-1.0, 0.0)), Home()]


def test_target_off_center_is_stepped_toward():
    # Act / Assert
    assert PanPlanner(TAPO).decide(Position(0.9, 0.5), 0.0) == STEP_RIGHT
