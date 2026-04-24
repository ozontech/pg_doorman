use std::time::Duration;

use futures::future::select_all;
use log::{debug, error, warn};

use super::types::ClusterResponse;

#[derive(Debug)]
pub enum PatroniError {
    AllUrlsFailed(Vec<(String, String)>),
}

impl std::fmt::Display for PatroniError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PatroniError::AllUrlsFailed(errors) => {
                write!(f, "all patroni urls failed:")?;
                for (url, err) in errors {
                    write!(f, " {url}: {err};")?;
                }
                Ok(())
            }
        }
    }
}

pub struct PatroniClient {
    http: reqwest::Client,
}

impl PatroniClient {
    pub fn new(request_timeout: Duration, connect_timeout: Duration) -> Self {
        let http = reqwest::Client::builder()
            .timeout(request_timeout)
            .connect_timeout(connect_timeout)
            .no_proxy()
            .build()
            .expect("failed to build reqwest client");
        Self { http }
    }

    /// Fetch /cluster from all URLs in parallel.
    /// Returns first successful response, lets the rest complete via their own timeouts.
    pub async fn fetch_cluster(&self, urls: &[String]) -> Result<ClusterResponse, PatroniError> {
        if urls.is_empty() {
            return Err(PatroniError::AllUrlsFailed(vec![]));
        }

        // Each future resolves to (url, Result<ClusterResponse, String>) so we always
        // have the originating URL regardless of success or failure.
        let futs: Vec<_> = urls
            .iter()
            .map(|url| {
                let request_url = format!("{}/cluster", url.trim_end_matches('/'));
                let http = self.http.clone();
                let url_owned = url.clone();
                Box::pin(async move {
                    debug!("fetching /cluster from {}", request_url);
                    let outcome: Result<ClusterResponse, String> = async {
                        let resp = http
                            .get(&request_url)
                            .send()
                            .await
                            .map_err(|e| format!("{e}"))?;

                        if !resp.status().is_success() {
                            return Err(format!("HTTP {}", resp.status()));
                        }

                        resp.json::<ClusterResponse>()
                            .await
                            .map_err(|e| format!("json parse: {e}"))
                    }
                    .await;

                    (url_owned, outcome)
                })
            })
            .collect();

        let mut remaining = futs;
        let mut errors: Vec<(String, String)> = Vec::new();

        while !remaining.is_empty() {
            let ((url, outcome), _idx, rest) = select_all(remaining).await;

            match outcome {
                Ok(cluster) => {
                    debug!(
                        "got /cluster from {}: {} members",
                        url,
                        cluster.members.len()
                    );
                    // Dropping `rest` cancels the remaining in-flight futures.
                    // reqwest respects its own timeouts for any leaked tasks.
                    return Ok(cluster);
                }
                Err(e) => {
                    warn!("patroni url {} failed: {}", url, e);
                    errors.push((url, e));
                }
            }

            remaining = rest;
        }

        error!("all patroni discovery urls failed");
        Err(PatroniError::AllUrlsFailed(errors))
    }
}
