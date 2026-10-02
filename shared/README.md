# shared

Crates used by more than one component.

- [`mediapack/`](mediapack/README.md): container demuxers and muxers (MPEG-TS, FLV, fMP4, LOC) and H.264/AAC bitstream helpers
- [`media-streaming-format/`](media-streaming-format/README.md): MSF catalog and media timeline types
- [`media-publisher/`](media-publisher/README.md): catalog, LOC/CMAF track publishing and FETCH cache shared by the publishers
- [`transcode/`](transcode/README.md): GStreamer-backed re-encoding of video into lower renditions
