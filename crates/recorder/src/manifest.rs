use std::path::PathBuf;

use anyhow::{Context, Result};
use mediapack::Timestamp;
use object_store::path::Path;
use serde_json::{Value, json};

use crate::composite::MemberFiles;

/// The object under a room recording's prefix that lists every uploaded file
/// by participant, with its capture origin: the Unix time in microseconds
/// that presentation time 0 of the file stands for. A composite places the
/// files on one timeline from it.
pub(crate) const MANIFEST_NAME: &str = "manifest.json";

/// Leaves out the files that hold no media, were not uploaded or whose upload
/// failed, and the participants left with none.
pub(crate) fn manifest_json(members: &[MemberFiles], failed_uploads: &[PathBuf]) -> Value {
    let members: Vec<Value> = members
        .iter()
        .filter_map(|member| {
            let files: Vec<Value> = member
                .files
                .iter()
                .filter(|file| !failed_uploads.contains(&file.path))
                .filter_map(|file| {
                    Some(json!({
                        "key": file.key.as_ref()?.as_ref(),
                        "capture_origin_us": file.capture_origin?.micros(),
                    }))
                })
                .collect();
            (!files.is_empty()).then(|| json!({ "namespace": member.namespace, "files": files }))
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
}

pub(crate) fn parse_manifest(manifest: &Value) -> Result<Vec<ManifestMember>> {
    let members = manifest["members"]
        .as_array()
        .context("manifest without members")?;
    members
        .iter()
        .map(|member| {
            let files = member["files"]
                .as_array()
                .context("member without files")?
                .iter()
                .map(|file| {
                    let key = file["key"].as_str().context("file without key")?;
                    let origin = file["capture_origin_us"]
                        .as_u64()
                        .context("file without capture_origin_us")?;
                    Ok((Path::from(key), Timestamp::from_micros(origin)))
                })
                .collect::<Result<_>>()?;
            Ok(ManifestMember {
                namespace: member["namespace"]
                    .as_str()
                    .context("member without namespace")?
                    .to_string(),
                files,
            })
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
            },
            MemberFiles {
                namespace: "anon/room/b".to_string(),
                files: vec![file("b-1.mp4", Some("1/anon/room/b/20-1.mp4"), Some(2_000))],
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
            }]
        );
    }
}
