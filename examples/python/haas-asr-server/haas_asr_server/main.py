import argparse

from aiohttp import web

from haas_asr_server.model import HaasTranscriber
from haas_asr_server.server import app


def main():
    parser = argparse.ArgumentParser(description="Serve Haas-ja over the OpenAI-compatible transcription API")
    parser.add_argument(
        "--model", default="third-intelligence/Haas-ja-v1.0", help="a Hugging Face model id or a .nemo file"
    )
    parser.add_argument("--host", default="127.0.0.1")
    parser.add_argument("--port", type=int, default=8080)
    args = parser.parse_args()
    # Loading before listening keeps /health failing, and Cloud Run's startup probe waiting, until the
    # model can answer.
    web.run_app(app(HaasTranscriber(args.model)), host=args.host, port=args.port)

