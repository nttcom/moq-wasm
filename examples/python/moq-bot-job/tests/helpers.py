from dataclasses import dataclass, field

from aiohttp import web


@dataclass
class FakeMetadataServer:
    url: str = ""
    requests: list[tuple[str | None, str | None]] = field(default_factory=list)

    async def handle(self, request: web.Request) -> web.Response:
        self.requests.append((request.headers.get("Metadata-Flavor"), request.query.get("audience")))
        return web.Response(text="metadata-token")
