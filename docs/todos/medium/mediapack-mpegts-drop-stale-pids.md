# [S] Drop the MPEG-TS streams of PIDs a newer PMT no longer lists

Next: in `register_streams` (`crates/mediapack/src/mpegts/demuxer.rs:156`), remove entries of `self.streams` whose PID is not in the new PMT.

## Goal

After a PMT change, only the PIDs it lists have a `PesStream`. Today `entry().or_insert_with` (`demuxer.rs:170`) only adds, so streams of dropped PIDs stay (up to ~8191 PIDs), and a PID the PMT re-maps to another stream type keeps its old `kind`.
