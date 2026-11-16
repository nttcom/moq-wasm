from moq_wasm.media import Catalog, CatalogTrack


def camera_video_track(catalog: Catalog) -> CatalogTrack | None:
    """The LOC video track the camera page publishes: the shortest-named one, as a rendition is named after its video."""
    video = [track for track in catalog.tracks if track.role == "video" and track.packaging == "loc"]
    if not video:
        return None
    return min(video, key=lambda track: len(track.name))
