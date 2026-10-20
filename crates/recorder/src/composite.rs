use std::{
    collections::VecDeque,
    fs::File,
    io::Read,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result, bail};
use mediapack::{MediaEvent, StreamSet, Timestamp, mp4::Demuxer};
use transcode::{CompositeInput, CompositeSettings, Compositor, EventSource, Loudness};

use crate::recording::RecordedFile;

const READ_CHUNK_BYTES: usize = 1 << 20;

/// The files one participant's recordings wrote, each list in recording
/// order.
#[derive(Debug, PartialEq)]
pub(crate) struct MemberFiles {
    pub(crate) namespace: String,
    /// The camera and the audio.
    pub(crate) files: Vec<RecordedFile>,
    pub(crate) screen_share: Vec<RecordedFile>,
}

/// Composites the room into `output` and returns the Unix time its
/// presentation time 0 stands for: the earliest capture origin of any file.
/// Each file is shifted by its own capture origin minus that one. Fails when
/// no file holds media.
///
/// Reads every participant's audio once more first, in parallel, to find
/// the active speaker.
pub(crate) async fn composite_room(members: &[MemberFiles], output: &Path) -> Result<Timestamp> {
    let Some(origin) = members
        .iter()
        .flat_map(|member| member.files.iter().chain(&member.screen_share))
        .filter_map(|file| file.capture_origin)
        .min()
    else {
        bail!("the room recording wrote no media");
    };
    let mut placed = Vec::with_capacity(members.len());
    for member in members {
        let (video, audio) = placed_files(&member.files, origin).await?;
        let (screen_share, _) = placed_files(&member.screen_share, origin).await?;
        if !video.is_empty() || !audio.is_empty() || !screen_share.is_empty() {
            placed.push((video, audio, screen_share));
        }
    }
    let measuring: Vec<_> = placed
        .iter()
        .map(|(_, audio, _)| {
            let audio = events(audio.clone(), Kind::Audio);
            tokio::task::spawn_blocking(move || audio.map(Loudness::measure).transpose())
        })
        .collect();
    let mut inputs = Vec::with_capacity(placed.len());
    for ((video, audio, screen_share), loudness) in placed.into_iter().zip(measuring) {
        inputs.push(CompositeInput {
            video: events(video, Kind::Video),
            screen_share: events(screen_share, Kind::Video),
            audio: events(audio, Kind::Audio),
            loudness: loudness
                .await?
                .context("measure a participant's loudness")?,
        });
    }
    let output = output.to_path_buf();
    let mut compositor = tokio::task::spawn_blocking(move || {
        Compositor::start(inputs, CompositeSettings::default(), &output)
    })
    .await??;
    compositor.finished().await?;
    Ok(origin)
}

type PlacedFiles = Vec<(PathBuf, Timestamp)>;

/// The files of `files` that hold video and those that hold audio, each
/// with its shift onto the room's timeline from `origin`.
async fn placed_files(
    files: &[RecordedFile],
    origin: Timestamp,
) -> Result<(PlacedFiles, PlacedFiles)> {
    let mut video = Vec::new();
    let mut audio = Vec::new();
    for file in files {
        let Some(file_origin) = file.capture_origin else {
            continue;
        };
        let shift = file_origin.saturating_sub(origin);
        let path = file.path.clone();
        let streams = tokio::task::spawn_blocking(move || read_streams(&path)).await??;
        if streams.has_video {
            video.push((file.path.clone(), shift));
        }
        if streams.has_audio {
            audio.push((file.path.clone(), shift));
        }
    }
    Ok((video, audio))
}

#[derive(Clone, Copy, PartialEq)]
enum Kind {
    Video,
    Audio,
}

fn events(files: PlacedFiles, kind: Kind) -> Option<EventSource> {
    (!files.is_empty()).then(|| {
        Box::new(FileEvents {
            files: files.into(),
            current: None,
            pending: VecDeque::new(),
            kind,
            buffer: vec![0; READ_CHUNK_BYTES],
        }) as EventSource
    })
}

/// The tracks a recorded file holds, from its init segment.
fn read_streams(path: &Path) -> Result<StreamSet> {
    let mut file = File::open(path).with_context(|| format!("cannot open {}", path.display()))?;
    let mut demuxer = Demuxer::new();
    let mut buffer = vec![0; READ_CHUNK_BYTES];
    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            bail!("{} has no init segment", path.display());
        }
        for event in demuxer.push(&buffer[..read])? {
            if let MediaEvent::Streams(streams) = event {
                return Ok(streams);
            }
        }
    }
}

struct OpenFile {
    path: PathBuf,
    file: File,
    demuxer: Demuxer,
    shift: Timestamp,
}

/// One kind of media of several files, read in order on the compositor's
/// feeder thread and shifted onto the room's timeline.
struct FileEvents {
    files: VecDeque<(PathBuf, Timestamp)>,
    current: Option<OpenFile>,
    pending: VecDeque<MediaEvent>,
    kind: Kind,
    buffer: Vec<u8>,
}

impl Iterator for FileEvents {
    type Item = Result<MediaEvent>;

    fn next(&mut self) -> Option<Self::Item> {
        loop {
            if let Some(event) = self.pending.pop_front() {
                return Some(Ok(event));
            }
            match self.read_more() {
                Ok(true) => {}
                Ok(false) => return None,
                Err(error) => {
                    self.files.clear();
                    self.current = None;
                    return Some(Err(error));
                }
            }
        }
    }
}

impl FileEvents {
    /// Returns false once every file has been read.
    fn read_more(&mut self) -> Result<bool> {
        let Some(current) = &mut self.current else {
            let Some((path, shift)) = self.files.pop_front() else {
                return Ok(false);
            };
            let file =
                File::open(&path).with_context(|| format!("cannot open {}", path.display()))?;
            self.current = Some(OpenFile {
                path,
                file,
                demuxer: Demuxer::new(),
                shift,
            });
            return Ok(true);
        };
        let shift = current.shift;
        let read = current.file.read(&mut self.buffer)?;
        let events = if read == 0 {
            // A recorder killed mid-way leaves a fragment cut short; the
            // fragments before it are complete.
            let events = current.demuxer.finish().unwrap_or_else(|error| {
                tracing::warn!(path = %current.path.display(), %error, "recorded file ends early");
                Vec::new()
            });
            self.current = None;
            events
        } else {
            current
                .demuxer
                .push(&self.buffer[..read])
                .with_context(|| format!("cannot read {}", current.path.display()))?
        };
        let kind = self.kind;
        self.pending.extend(
            events
                .into_iter()
                .filter_map(|event| shifted(event, kind, shift)),
        );
        Ok(true)
    }
}

/// `event` moved by `shift` when it is of `kind`.
fn shifted(event: MediaEvent, kind: Kind, shift: Timestamp) -> Option<MediaEvent> {
    match (event, kind) {
        (event @ MediaEvent::VideoConfig(_), Kind::Video)
        | (event @ MediaEvent::AudioConfig(_), Kind::Audio) => Some(event),
        (MediaEvent::Video(mut sample), Kind::Video) => {
            sample.pts = sample.pts.saturating_add(shift);
            sample.dts = sample.dts.saturating_add(shift);
            Some(MediaEvent::Video(sample))
        }
        (MediaEvent::Audio(mut sample), Kind::Audio) => {
            sample.pts = sample.pts.saturating_add(shift);
            Some(MediaEvent::Audio(sample))
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use mediapack::{AudioConfig, mp4::Mp4Index};

    use super::*;
    use crate::test_support::{scratch_dir, write_fixture_fmp4};

    const ORIGIN: Timestamp = Timestamp::from_micros(1_700_000_000_000_000);

    /// The progressive index of the file's top-level `moov`.
    fn read_index(path: &Path) -> Mp4Index {
        let file = std::fs::read(path).unwrap();
        let mut position = 0;
        while position + 8 <= file.len() {
            let size =
                u32::from_be_bytes(file[position..position + 4].try_into().unwrap()) as usize;
            if &file[position + 4..position + 8] == b"moov" {
                return Mp4Index::parse(&file[position..position + size]).unwrap();
            }
            position += size;
        }
        panic!("{} has no moov", path.display());
    }

    #[tokio::test]
    async fn composites_participants_on_the_room_timeline() {
        // Arrange: the second participant's recording started 2 s after the first
        let dir = scratch_dir();
        let (first, second) = (dir.join("first.mp4"), dir.join("second.mp4"));
        write_fixture_fmp4(&first);
        write_fixture_fmp4(&second);
        let member = |namespace: &str, path: &PathBuf, origin: Timestamp| MemberFiles {
            namespace: namespace.to_string(),
            files: vec![RecordedFile {
                path: path.clone(),
                key: None,
                capture_origin: Some(origin),
            }],
            screen_share: Vec::new(),
        };
        let members = [
            member(
                "anon/room/b",
                &second,
                ORIGIN.saturating_add(Timestamp::from_millis(2_000)),
            ),
            member("anon/room/a", &first, ORIGIN),
        ];
        let output = dir.join("composite.mp4");

        // Act
        let origin = composite_room(&members, &output).await.unwrap();

        // Assert: the fixture's video ends near 2.1 s, so the later participant's near 4.1 s
        let index = read_index(&output);
        std::fs::remove_dir_all(&dir).unwrap();
        assert_eq!(origin, ORIGIN);
        assert!(index.video.is_some());
        assert!(matches!(index.audio, Some(AudioConfig::Aac(_))));
        let duration = index.duration().millis();
        assert!((3_900..4_300).contains(&duration), "duration {duration} ms");
    }

    #[tokio::test]
    async fn composites_a_screen_share_on_the_room_timeline() {
        // Arrange: the participant starts sharing the screen 3 s after the recording
        let dir = scratch_dir();
        let (camera, share) = (dir.join("camera.mp4"), dir.join("share.mp4"));
        write_fixture_fmp4(&camera);
        write_fixture_fmp4(&share);
        let recorded = |path: &PathBuf, origin: Timestamp| RecordedFile {
            path: path.clone(),
            key: None,
            capture_origin: Some(origin),
        };
        let members = [MemberFiles {
            namespace: "anon/room/a".to_string(),
            files: vec![recorded(&camera, ORIGIN)],
            screen_share: vec![recorded(
                &share,
                ORIGIN.saturating_add(Timestamp::from_millis(3_000)),
            )],
        }];
        let output = dir.join("composite.mp4");

        // Act
        let origin = composite_room(&members, &output).await.unwrap();

        // Assert: the screen share's video ends near 5.1 s
        let index = read_index(&output);
        std::fs::remove_dir_all(&dir).unwrap();
        assert_eq!(origin, ORIGIN);
        let duration = index.duration().millis();
        assert!((4_900..5_300).contains(&duration), "duration {duration} ms");
    }

    #[tokio::test]
    async fn refuses_a_room_without_media() {
        // Arrange
        let members = [MemberFiles {
            namespace: "anon/room/a".to_string(),
            files: vec![RecordedFile {
                path: PathBuf::from("never-written.mp4"),
                key: None,
                capture_origin: None,
            }],
            screen_share: Vec::new(),
        }];

        // Act / Assert
        assert!(
            composite_room(&members, Path::new("unused.mp4"))
                .await
                .is_err()
        );
    }
}
