import asyncio
import time

# gcloud identity tokens expire after an hour; refresh well before that.
TOKEN_LIFETIME_SECONDS = 50 * 60
GCLOUD_COMMAND = ("gcloud", "auth", "print-identity-token")


class GcloudIdentityToken:
    def __init__(self, command: tuple[str, ...] = GCLOUD_COMMAND):
        self._command = command
        self._token: str | None = None
        self._fetched_at = 0.0

    async def value(self) -> str:
        if self._token is None or time.monotonic() - self._fetched_at > TOKEN_LIFETIME_SECONDS:
            process = await asyncio.create_subprocess_exec(
                *self._command, stdout=asyncio.subprocess.PIPE
            )
            stdout, _ = await process.communicate()
            if process.returncode != 0:
                raise RuntimeError(f"{' '.join(self._command)} exited with {process.returncode}")
            self._token = stdout.decode().strip()
            self._fetched_at = time.monotonic()
        return self._token
