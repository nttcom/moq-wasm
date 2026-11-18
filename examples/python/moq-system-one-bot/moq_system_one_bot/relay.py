import argparse
import asyncio
from collections.abc import Awaitable, Callable

import moq_wasm
from loguru import logger
from moq_bot_job.bot_job import Bots

RECONNECT_DELAY_SECONDS = 2.0


async def stay_connected(
    args: argparse.Namespace, bots: Bots, in_session: Callable[[moq_wasm.Session], Awaitable[None]]
) -> None:
    """Runs `in_session` on a relay session with the bot's token until it returns. Retries the first
    connection, and runs `in_session` again on a new session when `in_session` raises
    RuntimeError. Raises what ended the session, such as `moq_wasm.ConnectionLost` or
    `moq_wasm.TokenExpired`."""
    token = await bots.token()
    while True:
        try:
            session = await moq_wasm.connect(
                args.relay_url,
                verify_certificate=not args.insecure,
                token=token,
                refresh_token=bots.refresh if token else None,
            )
        except RuntimeError as error:
            logger.warning(f"cannot connect to {args.relay_url}: {error}")
            await asyncio.sleep(RECONNECT_DELAY_SECONDS)
            continue
        logger.info(f"connected to {args.relay_url}")
        try:
            await session.run(in_session(session))
            return
        except RuntimeError as error:
            logger.warning(f"relay session failed: {error}")
        finally:
            await session.close()
        await asyncio.sleep(RECONNECT_DELAY_SECONDS)
