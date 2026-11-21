import asyncio
import base64
import json
import logging
import time
from collections.abc import Awaitable, Callable
from dataclasses import dataclass
from typing import TypeVar

from moq_wasm._errors import SessionEnded
from moq_wasm._once import retrieve

logger = logging.getLogger("moq_wasm")

T = TypeVar("T")

# A token is refreshed this long before it expires, as the pages do (examples/browser/utils/access.ts),
# or after half its lifetime when it is shorter than twice this.
REFRESH_MARGIN_SECONDS = 5 * 60
# A failed refresh is retried after this long, or after half the time left when that is shorter,
# but no sooner than MIN_RETRY_SECONDS.
RETRY_SECONDS = 30.0
MIN_RETRY_SECONDS = 1.0
# How long one call of `refresh_token` may take before it counts as failed.
REFRESH_TIMEOUT_SECONDS = 30.0

RefreshToken = Callable[[str], Awaitable[str]]


@dataclass(frozen=True)
class TokenClaims:
    app_id: str
    # Unix seconds; None for a token that does not expire.
    expires_at: float | None


def read_claims(token: str) -> TokenClaims:
    """Reads `appId` and `exp` from a JWT without verifying it: the relay verifies the token, and
    the session only needs to know when to refresh it. Raises ValueError for what is not a JWT."""
    try:
        payload = token.split(".")[1]
        claims = json.loads(base64.urlsafe_b64decode(payload + "=" * (-len(payload) % 4)))
        return TokenClaims(app_id=claims["appId"], expires_at=claims.get("exp"))
    except (IndexError, KeyError, TypeError, ValueError) as error:
        raise ValueError(f"not a token with an appId claim: {error}") from error


class TokenExpired(SessionEnded):
    """The session's token expired before `refresh_token` returned a later one."""


class TokenKeeper:
    def __init__(self, token: str, refresh: RefreshToken):
        self.token = token
        self._claims = read_claims(token)
        self._refresh = refresh
        self._refresh_at = self._due(self._claims)
        # The calls of `refresh` and of the handover still running, those abandoned at their
        # deadline included.
        self._calls: set[asyncio.Task] = set()

    @property
    def app_id(self) -> str:
        return self._claims.app_id

    async def keep_fresh(self, hand_over: Callable[[str], Awaitable[None]]) -> None:
        """Refreshes the token `REFRESH_MARGIN_SECONDS` before it expires, or halfway through a
        shorter lifetime, and calls `hand_over` with each new one, until cancelled. A `refresh` and
        its `hand_over` share one deadline, `REFRESH_TIMEOUT_SECONDS` and no later than the token's
        expiry, which `_call` holds. A `refresh` or `hand_over` that raises, times out or ends
        cancelled while the keeper is not, as when the relay refuses the token, keeps the token the
        keeper had and is retried. Returns at once for a token that does not expire. Raises
        `TokenExpired` once the token expired while `refresh` kept failing. Cancelled, it ends at
        once and hands nothing over; `stop()` ends the calls it leaves."""
        while self._refresh_at is not None:
            await asyncio.sleep(max(self._refresh_at - time.time(), 0))
            await self._renew(hand_over)

    def runs(self, task: asyncio.Task | None) -> bool:
        """Whether `task` is a call of `refresh` or `hand_over` the keeper started."""
        return task in self._calls

    async def stop(self, sparing: asyncio.Task | None = None) -> None:
        """Cancels the calls still running, other than `sparing`, and waits until they finished
        their cleanup. One cancelled at its deadline is not cancelled again, which would interrupt
        its cleanup."""
        calls = [call for call in self._calls if call is not sparing]
        for call in calls:
            if not call.cancelling():
                call.cancel()
        if calls:
            await asyncio.wait(calls)

    async def _renew(self, hand_over: Callable[[str], Awaitable[None]]) -> None:
        expires_at = self._claims.expires_at
        assert expires_at is not None
        while True:
            left = expires_at - time.time()
            if left <= 0:
                raise TokenExpired(f"app {self.app_id}: the token expired")
            # One deadline for the call and the handover, no later than the token's expiry.
            deadline = asyncio.get_running_loop().time() + min(REFRESH_TIMEOUT_SECONDS, left)
            try:
                token = await self._call(lambda: self._refresh(self.token), deadline, "refresh_token")
                claims = self._later(read_claims(token))
                await self._call(lambda: hand_over(token), deadline, "handing the token over")
                self._claims = claims
                self.token = token
                self._refresh_at = self._due(claims)
                return
            except Exception as error:
                left = expires_at - time.time()
                if left <= 0:
                    raise TokenExpired(f"app {self.app_id}: the token expired: {error!r}") from error
                logger.warning("app %s: cannot refresh the token yet: %r", self.app_id, error)
                await asyncio.sleep(min(max(min(RETRY_SECONDS, left / 2), MIN_RETRY_SECONDS), left))

    async def _call(self, call: Callable[[], Awaitable[T]], deadline: float, what: str) -> T:
        """`call()` in a task of its own, so the keeper's cancellation, which raises from here at
        once, is told apart from the call's, and the deadline holds whatever the call's cleanup
        does. Raises TimeoutError at the deadline, leaving the call cancelled to finish on its own,
        and RuntimeError for a call that ended cancelled. The deadline is checked in the task, just
        before `call()` starts, and once it returned, as the event loop may have stalled past it
        meanwhile, also between scheduling the task and its start: a call it finds past its
        deadline is not started, or its result discarded."""
        loop = asyncio.get_running_loop()
        task = asyncio.ensure_future(_started_before(deadline, call, what))
        self._calls.add(task)
        task.add_done_callback(self._calls.discard)
        # What an abandoned call raises goes to nobody.
        task.add_done_callback(retrieve)
        await asyncio.wait([task], timeout=max(deadline - loop.time(), 0))
        if not task.done():
            task.cancel()
            raise TimeoutError(f"{what} did not return before its deadline")
        if loop.time() >= deadline:
            raise TimeoutError(f"{what} returned past its deadline")
        if task.cancelled():
            raise RuntimeError(f"{what} was cancelled")
        return task.result()

    def _later(self, claims: TokenClaims) -> TokenClaims:
        """`claims` of a refreshed token; raises ValueError unless it is of the same app and expires
        later, as a provider that hands back its cached token does not."""
        if claims.app_id != self._claims.app_id:
            raise ValueError(f"the refreshed token is of app {claims.app_id}")
        current = self._claims.expires_at
        if claims.expires_at is not None and current is not None and claims.expires_at <= current:
            raise ValueError("the refreshed token does not expire later than the current one")
        return claims

    @staticmethod
    def _due(claims: TokenClaims) -> float | None:
        if claims.expires_at is None:
            return None
        lifetime = max(claims.expires_at - time.time(), 0)
        return claims.expires_at - min(REFRESH_MARGIN_SECONDS, lifetime / 2)


async def _started_before(deadline: float, call: Callable[[], Awaitable[T]], what: str) -> T:
    """`call()`, unless the event loop's time reached `deadline` first."""
    if asyncio.get_running_loop().time() >= deadline:
        raise TimeoutError(f"{what} did not start before its deadline")
    return await call()
