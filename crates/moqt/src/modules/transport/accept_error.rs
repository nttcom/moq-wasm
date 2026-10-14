use thiserror::Error;

#[derive(Debug, Error)]
#[non_exhaustive]
pub enum AcceptError {
    /// Stop calling `accept`: the endpoint is closed and no further
    /// connection will arrive.
    #[error("endpoint is closing")]
    EndpointClosing,
    /// One connection attempt failed; keep calling `accept`.
    #[error(transparent)]
    Failed(#[from] anyhow::Error),
}
