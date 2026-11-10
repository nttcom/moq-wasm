# http-client

Builds the workspace's `reqwest` clients: `builder()` and `client()` replace
`reqwest::Client::builder` and `reqwest::Client::new`, which `clippy.toml`
disallows everywhere else, so a setting every client needs is made in one
place.
