# mediapack architecture

`crates/mediapack/README.md` describes the formats, the `MediaEvent` model
and the modules.

## Muxers kept ahead of their callers

`flv::Muxer`, `mpegts::Muxer` and `Transmuxer` have no caller in the
workspace's services yet: the ingest bridges only demux FLV and MPEG-TS, and
the recorder only writes fragmented MP4. They stay because FLV and MPEG-TS
output is planned.
