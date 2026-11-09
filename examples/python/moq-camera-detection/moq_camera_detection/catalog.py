import json


def camera_video_track(catalog: bytes) -> str | None:
    """The LOC video track the camera page publishes: the shortest-named one, as a rendition is named after its video."""
    tracks = json.loads(catalog).get("tracks", [])
    video = [track["name"] for track in tracks if track.get("role") == "video" and track.get("packaging") == "loc"]
    if not video:
        return None
    return min(video, key=len)
