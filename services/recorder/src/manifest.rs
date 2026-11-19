use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::{composite::MemberFiles, recording::RecordedFile};

/// The object under a room recording's prefix that lists every uploaded file
/// by participant: camera and audio in `files`, screen share in
/// `screen_share_files`. Each file carries its capture origin, the Unix time
/// in microseconds of its presentation time 0, which places it on the
/// composite's timeline.
pub(crate) const MANIFEST_NAME: &str = "manifest.json";

#[derive(Debug, PartialEq, Serialize, Deserialize)]
pub(crate) struct Manifest {
    pub(crate) members: Vec<ManifestMember>,
}

#[derive(Debug, PartialEq, Serialize, Deserialize)]
pub(crate) struct ManifestMember {
    pub(crate) namespace: String,
    pub(crate) files: Vec<ManifestFile>,
    /// Empty in a manifest written before screen shares were recorded.
    #[serde(rename = "screen_share_files", default)]
    pub(crate) screen_share: Vec<ManifestFile>,
}

#[derive(Debug, PartialEq, Serialize, Deserialize)]
pub(crate) struct ManifestFile {
    pub(crate) key: String,
    pub(crate) capture_origin_us: u64,
}

/// Leaves out the files that hold no media, were not uploaded or whose upload
/// failed, and the participants left with none.
pub(crate) fn manifest(members: &[MemberFiles], failed_uploads: &[PathBuf]) -> Manifest {
    let listed = |files: &[RecordedFile]| -> Vec<ManifestFile> {
        files
            .iter()
            .filter(|file| !failed_uploads.contains(&file.path))
            .filter_map(|file| {
                Some(ManifestFile {
                    key: file.key.as_ref()?.to_string(),
                    capture_origin_us: file.capture_origin?.micros(),
                })
            })
            .collect()
    };
    let members = members
        .iter()
        .filter_map(|member| {
            let files = listed(&member.files);
            let screen_share = listed(&member.screen_share);
            (!files.is_empty() || !screen_share.is_empty()).then(|| ManifestMember {
                namespace: member.namespace.clone(),
                files,
                screen_share,
            })
        })
        .collect();
    Manifest { members }
}

#[cfg(test)]
mod tests {
    use mediapack::Timestamp;
    use object_store::path::Path;
    use serde_json::{from_value, json, to_value};

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
        let manifest = to_value(manifest(&members, &[PathBuf::from("b-1.mp4")])).unwrap();

        // Assert
        assert_eq!(
            manifest.to_string(),
            concat!(
                r#"{"members":[{"files":[{"capture_origin_us":1000,"key":"1/anon/room/a/10-1.mp4"}],"#,
                r#""namespace":"anon/room/a","screen_share_files":[{"capture_origin_us":3000,"#,
                r#""key":"1/anon/room/a/screenshare/10-1.mp4"}]}]}"#,
            )
        );
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
            from_value::<Manifest>(manifest).unwrap().members,
            [ManifestMember {
                namespace: "anon/room/a".to_string(),
                files: vec![ManifestFile {
                    key: "1/anon/room/a/10-1.mp4".to_string(),
                    capture_origin_us: 1_000,
                }],
                screen_share: vec![ManifestFile {
                    key: "1/anon/room/a/screenshare/10-1.mp4".to_string(),
                    capture_origin_us: 3_000,
                }],
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
        let members = from_value::<Manifest>(manifest).unwrap().members;

        // Assert
        assert_eq!(members[0].files.len(), 1);
        assert!(members[0].screen_share.is_empty());
    }
}
