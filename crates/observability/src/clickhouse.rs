use anyhow::{Context, bail};
use serde::{Serialize, de::DeserializeOwned};

use crate::config::ClickHouseConfig;

#[derive(Clone)]
pub struct ClickHouse {
    http: reqwest::Client,
    config: ClickHouseConfig,
}

impl ClickHouse {
    pub fn new(config: ClickHouseConfig) -> Self {
        Self {
            http: reqwest::Client::new(),
            config,
        }
    }

    pub fn database(&self) -> &str {
        &self.config.database
    }

    pub async fn execute(&self, sql: &str) -> anyhow::Result<()> {
        self.post(&[], sql.to_string()).await.map(drop)
    }

    pub async fn insert<T: Serialize>(&self, table: &str, rows: &[T]) -> anyhow::Result<()> {
        if rows.is_empty() {
            return Ok(());
        }
        let mut body = String::new();
        for row in rows {
            body.push_str(&serde_json::to_string(row)?);
            body.push('\n');
        }
        let query = format!(
            "INSERT INTO {}.{table} FORMAT JSONEachRow",
            self.config.database
        );
        self.post(
            &[
                ("query", query.as_str()),
                ("async_insert", "1"),
                ("wait_for_async_insert", "1"),
                // Every relay inserts into each table once per second. A fixed one-second
                // window folds those inserts into one part per table per second; the adaptive
                // window shrinks under frequent inserts to about one part per insert.
                ("async_insert_use_adaptive_busy_timeout", "0"),
                ("async_insert_busy_timeout_max_ms", "1000"),
                ("input_format_skip_unknown_fields", "1"),
            ],
            body,
        )
        .await
        .map(drop)
    }

    pub async fn select<T: DeserializeOwned>(
        &self,
        sql: &str,
        params: &[(&str, String)],
    ) -> anyhow::Result<Vec<T>> {
        let params: Vec<(String, &str)> = params
            .iter()
            .map(|(name, value)| (format!("param_{name}"), value.as_str()))
            .collect();
        let params: Vec<(&str, &str)> = params
            .iter()
            .map(|(name, value)| (name.as_str(), *value))
            .collect();
        let body = self
            .post(&params, format!("{sql} FORMAT JSONEachRow"))
            .await?;
        body.lines()
            .filter(|line| !line.is_empty())
            .map(|line| {
                serde_json::from_str(line).with_context(|| format!("unexpected row {line}"))
            })
            .collect()
    }

    async fn post(&self, params: &[(&str, &str)], body: String) -> anyhow::Result<String> {
        let mut url =
            reqwest::Url::parse(&self.config.url).context("CLICKHOUSE_URL is not a URL")?;
        url.query_pairs_mut()
            .extend_pairs(params)
            .append_pair("output_format_json_quote_64bit_integers", "0");
        let mut request = self.http.post(url).body(body);
        if let Some(user) = &self.config.user {
            request = request.basic_auth(user, self.config.password.as_ref());
        }
        let response = request.send().await.context("ClickHouse is unreachable")?;
        let status = response.status();
        let text = response.text().await?;
        if !status.is_success() {
            bail!("ClickHouse answered {status}: {}", text.trim());
        }
        Ok(text)
    }
}
