import argparse

DEFAULT_RELAY_URL = "https://127.0.0.1:4433"


def add_relay_arguments(parser: argparse.ArgumentParser) -> None:
    parser.add_argument("--relay-url", default=DEFAULT_RELAY_URL)
    parser.add_argument(
        "--insecure",
        action="store_true",
        help="skip TLS verification, for a local relay with a self-signed certificate",
    )
