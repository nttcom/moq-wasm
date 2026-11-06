use std::path::{Path, PathBuf};

const INDEX: &str = "index.html";
const ASSETS: &str = "assets";

/// A file under the root, read whole; `immutable` marks a hashed asset under
/// `assets/`, which a browser may cache indefinitely.
pub struct StaticFile {
    pub bytes: Vec<u8>,
    pub content_type: &'static str,
    immutable: bool,
}

impl StaticFile {
    pub fn cache_control(&self) -> &'static str {
        if self.immutable {
            "public, max-age=31536000, immutable"
        } else {
            "no-cache"
        }
    }
}

/// Serves the files of a built page. A path is served only when every segment
/// is a plain file name, so nothing outside the root can be reached, and only
/// for the file types a page is made of. `/` is `index.html`.
pub struct StaticFiles {
    root: PathBuf,
}

impl StaticFiles {
    pub fn new(root: PathBuf) -> Self {
        Self { root }
    }

    pub async fn get(&self, uri_path: &str) -> Option<StaticFile> {
        let relative = relative_path(uri_path)?;
        let content_type = content_type(&relative)?;
        let bytes = tokio::fs::read(self.root.join(&relative)).await.ok()?;
        Some(StaticFile {
            bytes,
            content_type,
            immutable: relative.starts_with(ASSETS),
        })
    }
}

fn relative_path(uri_path: &str) -> Option<PathBuf> {
    let trimmed = uri_path.strip_prefix('/')?;
    if trimmed.is_empty() {
        return Some(PathBuf::from(INDEX));
    }
    let segments: Vec<&str> = trimmed.split('/').collect();
    segments
        .iter()
        .all(|segment| is_plain_segment(segment))
        .then(|| segments.iter().collect())
}

fn is_plain_segment(segment: &str) -> bool {
    !segment.is_empty()
        && !segment.starts_with('.')
        && segment.chars().all(|character| {
            character.is_ascii_alphanumeric() || matches!(character, '.' | '_' | '-')
        })
}

fn content_type(path: &Path) -> Option<&'static str> {
    Some(match path.extension()?.to_str()? {
        "html" => "text/html; charset=utf-8",
        "js" => "text/javascript; charset=utf-8",
        "css" => "text/css; charset=utf-8",
        "svg" => "image/svg+xml",
        "png" => "image/png",
        "ico" => "image/x-icon",
        "woff2" => "font/woff2",
        "wasm" => "application/wasm",
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_root_is_the_index() {
        // Act / Assert
        assert_eq!(relative_path("/"), Some(PathBuf::from("index.html")));
    }

    #[test]
    fn nested_asset_paths_are_kept() {
        // Act / Assert
        assert_eq!(
            relative_path("/assets/index-Bq3k_9.js"),
            Some(PathBuf::from("assets/index-Bq3k_9.js"))
        );
    }

    #[test]
    fn paths_that_could_leave_the_root_are_refused() {
        // Act / Assert
        for path in [
            "/../secret",
            "/assets/../../etc/passwd",
            "/.env",
            "//etc/passwd",
            "/a%2f..",
            "/assets/",
        ] {
            assert_eq!(relative_path(path), None, "{path}");
        }
    }

    #[test]
    fn only_known_file_types_are_served() {
        // Act
        let script = content_type(Path::new("assets/index.js"));
        let registry = content_type(Path::new("registry.json"));
        let without_extension = content_type(Path::new("Makefile"));

        // Assert
        assert_eq!(script, Some("text/javascript; charset=utf-8"));
        assert_eq!(registry, None);
        assert_eq!(without_extension, None);
    }
}
