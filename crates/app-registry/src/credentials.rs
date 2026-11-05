use metadata_token::MetadataToken;

pub(crate) enum Credentials {
    None,
    MetadataServer(MetadataToken),
}

impl Credentials {
    pub(crate) async fn bearer_token(&self) -> anyhow::Result<Option<String>> {
        match self {
            Self::None => Ok(None),
            Self::MetadataServer(token) => token.bearer_token().await.map(Some),
        }
    }
}
