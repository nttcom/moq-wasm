use std::path::PathBuf;

use anyhow::{Context, Result};
use mediapack::Timestamp;
use object_store::path::Path;
use serde_json::{Value, json};

use crate::{composite::MemberFiles, recording::RecordedFile};

/// The object under a room recording's prefix that lists every uploaded file
/// by participant: camera and audio in `files`, screen share in
/// `screen_share_files`. Each file carries its capture origin, the Unix time
/// in microseconds of its presentation time 0, which places it on the
/// composite's timeline.
pub(crate) const MANIFEST_NAME: &str = "manifest.json";

/// Leaves out the files that hold no media, were not uploaded or whose upload
/// failed, and the participants left with none.
pub(crate) fn manifest_json(members: &[MemberFiles], failed_uploads: &[PathBuf]) -> Value {
    let listed = |files: &[RecordedFile]| -> Vec<Value> {
        files
            .iter()
            .filter(|file| !failed_uploads.contains(&file.path))
            .filter_map(|file| {
                Some(json!({
                    "key": file.key.as_ref()?.as_ref(),
                    "capture_origin_us": file.capture_origin?.micros(),
                }))
            })
            .collect()
    };
    let members: Vec<Value> = members
        .iter()
        .filter_map(|member| {
            let files = listed(&member.files);
            let screen_share = listed(&member.screen_share);
            (!files.is_empty() || !screen_share.is_empty()).then(|| {
                json!({
                    "namespace": member.namespace,
                    "files": files,
                    "screen_share_files": screen_share,
                })
            })
        })
        .collect();
    json!({ "members": members })
}

/// One participant's files as the manifest lists them: object key and
/// capture origin.
#[derive(Debug, PartialEq)]
pub(crate) struct ManifestMember {
    pub(crate) namespace: String,
    pub(crate) files: Vec<(Path, Timestamp)>,
    pub(crate) screen_share: Vec<(Path, Timestamp)>,
}

/// Accepts a manifest without `screen_share_files`, written before screen
/// shares were recorded.
pub(crate) fn parse_manifest(manifest: &Value) -> Result<Vec<ManifestMember>> {
    let members = manifest["members"]
        .as_array()
        .context("manifest without members")?;
    members
        .iter()
        .map(|member| {
            let files = member["files"].as_array().context("member without files")?;
            let screen_share = match &member["screen_share_files"] {
                Value::Null => &Vec::new(),
                files => files
                    .as_array()
                    .context("screen_share_files is not a list")?,
            };
            Ok(ManifestMember {
                namespace: member["namespace"]
                    .as_str()
                    .context("member without namespace")?
                    .to_string(),
                files: parse_files(files)?,
                screen_share: parse_files(screen_share)?,
            })
        })
        .collect()
}

fn parse_files(files: &[Value]) -> Result<Vec<(Path, Timestamp)>> {
    files
        .iter()
        .map(|file| {
            let key = file["key"].as_str().context("file without key")?;
            let origin = file["capture_origin_us"]
                .as_u64()
                .context("file without capture_origin_us")?;
            Ok((Path::from(key), Timestamp::from_micros(origin)))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::recording::RecordedFile;

    fn file(name: &str, key: Option<&str>, origin_us: Option<u64>) -> RecordedFile {
        RecordedFile {
            path: PathBuf::from(name),
            key: key.map(Path::from),
            capture_origin: origin_us.map(Timestamp::from_micros),
        }
    }

    #[test]
    fn lists_only_the_uploaded_files_with_media() {
        // Arrange: b's only upload failed, and a-2.mp4 holds no media
        let members = [
            MemberFiles {
                namespace: "anon/room/a".to_string(),
                files: vec![
                    file("a-1.mp4", Some("1/anon/room/a/10-1.mp4"), Some(1_000)),
                    file("a-2.mp4", Some("1/anon/room/a/10-2.mp4"), None),
                ],
                screen_share: vec![file(
                    "a-share.mp4",
                    Some("1/anon/room/a/screenshare/10-1.mp4"),
                    Some(3_000),
                )],
            },
            MemberFiles {
                namespace: "anon/room/b".to_string(),
                files: vec![file("b-1.mp4", Some("1/anon/room/b/20-1.mp4"), Some(2_000))],
                screen_share: Vec::new(),
            },
        ];

        // Act
        let manifest = manifest_json(&members, &[PathBuf::from("b-1.mp4")]);

        // Assert
        assert_eq!(
            manifest,
            json!({ "members": [{
                "namespace": "anon/room/a",
                "files": [{ "key": "1/anon/room/a/10-1.mp4", "capture_origin_us": 1_000 }],
                "screen_share_files": [
                    { "key": "1/anon/room/a/screenshare/10-1.mp4", "capture_origin_us": 3_000 },
                ],
            }] })
        );
        assert_eq!(
            parse_manifest(&manifest).unwrap(),
            [ManifestMember {
                namespace: "anon/room/a".to_string(),
                files: vec![(
                    Path::from("1/anon/room/a/10-1.mp4"),
                    Timestamp::from_micros(1_000)
                )],
                screen_share: vec![(
                    Path::from("1/anon/room/a/screenshare/10-1.mp4"),
                    Timestamp::from_micros(3_000)
                )],
            }]
        );
    }

    #[test]
    fn reads_a_manifest_without_screen_shares() {
        // Arrange
        let manifest = json!({ "members": [{
            "namespace": "anon/room/a",
            "files": [{ "key": "1/anon/room/a/10-1.mp4", "capture_origin_us": 1_000 }],
        }] });

        // Act
        let members = parse_manifest(&manifest).unwrap();

        // Assert
        assert_eq!(members[0].files.len(), 1);
        assert!(members[0].screen_share.is_empty());
    }
}
