import logging
from collections.abc import Iterator
from contextlib import contextmanager


class FailingHandler(logging.Handler):
    def emit(self, record: logging.LogRecord) -> None:
        raise RuntimeError("handler failed")


@contextmanager
def failing_handler(logger_name: str) -> Iterator[None]:
    """Gives `logger_name` a handler that raises for every record, and keeps its records from the
    root logger's handlers, until the block exits."""
    failing_logger = logging.getLogger(logger_name)
    failing = FailingHandler()
    failing_logger.addHandler(failing)
    failing_logger.propagate = False
    try:
        yield
    finally:
        failing_logger.removeHandler(failing)
        failing_logger.propagate = True
