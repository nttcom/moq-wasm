"""Stands in for onvif-ingest in the PTZ tracking E2E: streams a synthetic H.264 picture through
`moq-cli publish` as the camera's LOC track, and records the PTZ commands the bot publishes, served
as JSON at `GET /commands`. Runs in the moq-ptz-tracking environment (`uv run --project`)."""

import argparse
import asyncio
import itertools
import json
import os
from fractions import Fraction

import av
import moqt
from aiohttp import web

CAMERA_NAMESPACE = ["anon", "onvif", "client"]
CAMERA_TRACK = "/".join([*CAMERA_NAMESPACE, "video"])
COMMAND_NAMESPACE = ["anon", "onvif", "viewer"]
COMMAND_TRACK = "command"
WIDTH, HEIGHT, FPS = 320, 180, 10
GRAY = b"\x80"


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description="Fake onvif-ingest for the PTZ tracking E2E")
    parser.add_argument("--relay-url", required=True)
    parser.add_argument("--moq-cli", required=True, help="path of the moq-cli binary that publishes the camera")
    parser.add_argument("--port", type=int, required=True, help="HTTP port serving GET /commands")
    return parser.parse_args()


async def stream_camera(moq_cli: str, relay_url: str) -> None:
    publisher = await asyncio.create_subprocess_exec(
        moq_cli,
        "publish",
        "--relay",
        relay_url,
        "--track",
        CAMERA_TRACK,
        "--codec",
        "avc3",
        "--insecure",
        stdin=asyncio.subprocess.PIPE,
        env={**os.environ, "RUST_LOG": os.environ.get("RUST_LOG", "info")},
    )
    encoder = av.CodecContext.create("libx264", "w")
    encoder.width, encoder.height, encoder.pix_fmt = WIDTH, HEIGHT, "yuv420p"
    encoder.time_base = Fraction(1, FPS)
    encoder.gop_size = FPS
    encoder.options = {"tune": "zerolatency", "profile": "baseline"}
    picture = av.VideoFrame(WIDTH, HEIGHT, "yuv420p")
    for plane in picture.planes:
        plane.update(GRAY * plane.buffer_size)
    try:
        for pts in itertools.count():
            picture.pts = pts
            for packet in encoder.encode(picture):
                publisher.stdin.write(bytes(packet))
            await publisher.stdin.drain()
            await asyncio.sleep(1 / FPS)
    finally:
        publisher.terminate()
        await publisher.wait()


async def record_commands(session: moqt.Session, commands: list[dict]) -> None:
    async for event in await session.subscribe_namespace(COMMAND_NAMESPACE[:-1]):
        if event.namespace != COMMAND_NAMESPACE or not event.active:
            continue
        try:
            async for command in await session.subscribe(COMMAND_NAMESPACE, COMMAND_TRACK):
                commands.append(json.loads(command.payload))
        except RuntimeError as error:
            print(f"command track ended: {error}", flush=True)


async def main() -> None:
    args = parse_args()
    commands: list[dict] = []
    app = web.Application()
    app.router.add_get("/commands", lambda _request: web.json_response(commands))
    runner = web.AppRunner(app)
    await runner.setup()
    await web.TCPSite(runner, "127.0.0.1", args.port).start()
    session = await moqt.connect(args.relay_url, verify_certificate=False)
    # moq-cli sends PUBLISH only; the bot waits for the camera namespace to be announced.
    await session.publish_namespace(CAMERA_NAMESPACE)
    print(f"serving commands on http://127.0.0.1:{args.port}", flush=True)
    try:
        async with asyncio.TaskGroup() as tasks:
            tasks.create_task(stream_camera(args.moq_cli, args.relay_url))
            tasks.create_task(record_commands(session, commands))
    finally:
        await session.close()
        await runner.cleanup()


if __name__ == "__main__":
    asyncio.run(main())
