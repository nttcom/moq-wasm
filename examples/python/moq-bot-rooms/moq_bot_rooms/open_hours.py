import re
from dataclasses import dataclass
from datetime import datetime, time, timedelta, timezone

WEEKDAYS = ["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"]
HOURS_PATTERN = re.compile(r"(\w{3})-(\w{3}) (\d{2}):(\d{2})-(\d{2}):(\d{2}) ([+-])(\d{2}):(\d{2})")


@dataclass(frozen=True)
class OpenHours:
    """Weekly opening hours at a fixed UTC offset, such as `Mon-Fri 09:00-17:30 +09:00`."""

    first_weekday: int
    last_weekday: int
    opens: time
    closes: time
    zone: timezone

    @classmethod
    def parse(cls, text: str) -> "OpenHours":
        match = HOURS_PATTERN.fullmatch(text.strip())
        if not match or match[1] not in WEEKDAYS or match[2] not in WEEKDAYS:
            raise ValueError(f"expected hours like 'Mon-Fri 09:00-17:30 +09:00', got {text!r}")
        first_weekday, last_weekday = WEEKDAYS.index(match[1]), WEEKDAYS.index(match[2])
        opens = time(int(match[3]), int(match[4]))
        closes = time(int(match[5]), int(match[6]))
        if first_weekday > last_weekday or opens >= closes:
            raise ValueError(f"opening hours must not wrap around: {text!r}")
        offset = timedelta(hours=int(match[8]), minutes=int(match[9]))
        zone = timezone(offset if match[7] == "+" else -offset)
        return cls(first_weekday, last_weekday, opens, closes, zone)

    def contains(self, moment: datetime) -> bool:
        local = moment.astimezone(self.zone)
        return self.first_weekday <= local.weekday() <= self.last_weekday and self.opens <= local.time() < self.closes
