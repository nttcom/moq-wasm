use std::path::Path;

use anyhow::{Context, Result, bail};

pub(super) async fn read_auth_token(path: &Path) -> Result<String> {
    let content = tokio::fs::read_to_string(path)
        .await
        .with_context(|| format!("failed to read auth token file {}", path.display()))?;
    let token = content.trim();
    if token.is_empty() {
        bail!("auth token file {} is empty", path.display());
    }
    Ok(token.to_string())
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::read_auth_token;

    fn token_file(name: &str, content: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!("moq-cli-{name}-{}", std::process::id()));
        std::fs::write(&path, content).unwrap();
        path
    }

    #[tokio::test]
    async fn returns_the_trimmed_content() {
        // Arrange
        let path = token_file("trimmed", "  jwt-value\n");

        // Act
        let token = read_auth_token(&path).await.unwrap();

        // Assert
        assert_eq!(token, "jwt-value");
    }

    #[tokio::test]
    async fn empty_file_is_an_error() {
        // Arrange
        let path = token_file("empty", "\n");

        // Act / Assert
        assert!(read_auth_token(&path).await.is_err());
    }

    #[tokio::test]
    async fn missing_file_is_an_error() {
        // Arrange
        let path = std::env::temp_dir().join("moq-cli-missing-token-file");

        // Act / Assert
        assert!(read_auth_token(&path).await.is_err());
    }
}
