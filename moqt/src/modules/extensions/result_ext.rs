pub(crate) trait ResultExt<T, E> {
    fn log_context<C: std::fmt::Display>(self, context: C) -> Result<T, E>;
}

impl<T, E: std::fmt::Display> ResultExt<T, E> for Result<T, E> {
    fn log_context<C: std::fmt::Display>(self, context: C) -> Result<T, E> {
        self.inspect_err(|e| {
            tracing::error!("{}: {}", context, e);
        })
    }
}
