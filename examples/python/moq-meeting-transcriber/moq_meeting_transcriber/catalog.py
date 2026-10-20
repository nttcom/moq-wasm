import json


def audio_track_name(catalog: bytes) -> str | None:
    """The Opus LOC audio track the Meeting's viewers subscribe to: the one with the highest bitrate.

    The publisher encodes a profile only while someone subscribes to it, so picking the same one
    starts no extra encoder in the participant's browser.
    """
    tracks = json.loads(catalog).get("tracks", [])
    audio = [
        track
        for track in tracks
        if track.get("role") == "audio" and track.get("packaging") == "loc" and track.get("codec") == "opus"
    ]
    if not audio:
        return None
    return max(audio, key=lambda track: track.get("bitrate", 0))["name"]
