import argparse

import aiohttp
from moq_bot_rooms.identity_token import IdentityToken, gcloud_identity_token, metadata_identity_token


def add_identity_token_arguments(parser: argparse.ArgumentParser) -> None:
    auth = parser.add_mutually_exclusive_group()
    auth.add_argument(
        "--gcloud-auth",
        action="store_true",
        help="send a gcloud identity token, for a Cloud Run service that requires IAM",
    )
    auth.add_argument(
        "--metadata-auth",
        action="store_true",
        help="send the metadata server's identity token, for a bot on Cloud Run or GCE",
    )


def identity_token_from_arguments(
    args: argparse.Namespace, session: aiohttp.ClientSession, service_url: str
) -> IdentityToken | None:
    if args.gcloud_auth:
        return gcloud_identity_token()
    if args.metadata_auth:
        return metadata_identity_token(session, service_url)
    return None
