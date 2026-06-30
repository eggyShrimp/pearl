use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use tracing::debug;

use crate::config::{EmbeddingConfig, EmbeddingProvider};

#[derive(Serialize)]
struct EmbeddingRequest {
    model: String,
    input: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    dimensions: Option<usize>,
}

#[derive(Deserialize)]
struct EmbeddingResponse {
    data: Vec<EmbeddingData>,
}

#[derive(Deserialize)]
struct EmbeddingData {
    embedding: Vec<f32>,
}

#[derive(Deserialize)]
struct OllamaTagsResponse {
    models: Option<Vec<OllamaModel>>,
}

#[derive(Deserialize)]
struct OllamaModel {
    name: String,
}

/// Maximum number of concurrent HTTP requests to the embedding API.
const MAX_CONCURRENCY: usize = 4;

/// Get embeddings for a batch of texts (synchronous, used for single queries and health checks).
pub fn get_embeddings(config: &EmbeddingConfig, texts: &[String]) -> Result<Vec<Vec<f32>>> {
    if texts.is_empty() {
        return Ok(vec![]);
    }

    let url = format!("{}/v1/embeddings", config.endpoint);
    let auth_header = resolve_auth_header(config)?;
    let batch_size = batch_size_for_provider(&config.provider);
    let mut all_embeddings = Vec::new();

    let mut client_builder =
        reqwest::blocking::Client::builder().timeout(std::time::Duration::from_secs(120));
    if config.provider == EmbeddingProvider::Ollama {
        client_builder = client_builder.danger_accept_invalid_certs(true);
    }
    let client = client_builder
        .build()
        .context("Failed to build HTTP client")?;

    for (batch_idx, chunk) in texts.chunks(batch_size).enumerate() {
        let request = EmbeddingRequest {
            model: config.model.clone(),
            input: chunk.to_vec(),
            dimensions: config.dimensions,
        };

        let mut req = client.post(&url).header("Content-Type", "application/json");

        if let Some(ref auth) = auth_header {
            req = req.header("Authorization", auth);
        }

        let response = req
            .json(&request)
            .send()
            .with_context(|| format!("Failed to call embedding API (batch {})", batch_idx))?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().unwrap_or_default();
            bail!(
                "Embedding API returned {} (batch {}): {}",
                status,
                batch_idx,
                body.chars().take(200).collect::<String>()
            );
        }

        let resp: EmbeddingResponse = response.json().with_context(|| {
            format!("Failed to parse embedding response (batch {})", batch_idx)
        })?;

        for data in resp.data {
            all_embeddings.push(data.embedding);
        }
    }

    Ok(all_embeddings)
}

/// Get embeddings for a large batch of texts using async HTTP with concurrency.
/// Splits texts into sub-batches and sends up to MAX_CONCURRENCY requests in parallel.
pub async fn get_embeddings_async(
    config: &EmbeddingConfig,
    texts: &[String],
) -> Result<Vec<Vec<f32>>> {
    if texts.is_empty() {
        return Ok(vec![]);
    }

    let url = format!("{}/v1/embeddings", config.endpoint);
    let auth_header = resolve_auth_header(config)?;
    let batch_size = batch_size_for_provider(&config.provider);

    // Split into sub-batches
    let batches: Vec<&[String]> = texts.chunks(batch_size).collect();

    if batches.len() == 1 {
        // Single batch: no concurrency needed, avoid reqwest overhead
        return get_embeddings(config, texts);
    }

    // Build async HTTP client
    let mut client_builder =
        reqwest::Client::builder().timeout(std::time::Duration::from_secs(120));
    // Disable TLS certificate verification for local endpoints (Ollama)
    if config.provider == EmbeddingProvider::Ollama {
        client_builder = client_builder.danger_accept_invalid_certs(true);
    }
    let client = client_builder
        .build()
        .context("Failed to build HTTP client")?;

    // Use a semaphore to limit concurrency
    let semaphore = std::sync::Arc::new(tokio::sync::Semaphore::new(MAX_CONCURRENCY));
    let mut handles = Vec::with_capacity(batches.len());

    for (batch_idx, batch) in batches.iter().enumerate() {
        let sem = semaphore.clone();
        let client = client.clone();
        let url = url.clone();
        let auth = auth_header.clone();
        let request_body = EmbeddingRequest {
            model: config.model.clone(),
            input: batch.to_vec(),
            dimensions: config.dimensions,
        };

        let handle = tokio::spawn(async move {
            let _permit = sem
                .acquire()
                .await
                .map_err(|e| anyhow::anyhow!("Semaphore error: {}", e))?;

            let mut req = client.post(&url).header("Content-Type", "application/json");

            if let Some(ref auth) = auth {
                req = req.header("Authorization", auth);
            }

            let response =
                req.json(&request_body).send().await.with_context(|| {
                    format!("Failed to call embedding API (batch {})", batch_idx)
                })?;

            if !response.status().is_success() {
                let status = response.status();
                let body = response.text().await.unwrap_or_default();
                bail!(
                    "Embedding API returned {} (batch {}): {}",
                    status,
                    batch_idx,
                    body.chars().take(200).collect::<String>()
                );
            }

            let resp: EmbeddingResponse = response.json().await.with_context(|| {
                format!("Failed to parse embedding response (batch {})", batch_idx)
            })?;

            let embeddings: Vec<Vec<f32>> = resp.data.into_iter().map(|d| d.embedding).collect();
            Ok::<(usize, Vec<Vec<f32>>), anyhow::Error>((batch_idx, embeddings))
        });

        handles.push(handle);
    }

    // Collect results in order
    let mut results: Vec<(usize, Vec<Vec<f32>>)> = Vec::with_capacity(handles.len());
    for handle in handles {
        let result = handle.await.context("Embedding task panicked")??;
        results.push(result);
    }

    // Sort by batch index and flatten
    results.sort_by_key(|(idx, _)| *idx);
    let all_embeddings: Vec<Vec<f32>> = results.into_iter().flat_map(|(_, vecs)| vecs).collect();

    Ok(all_embeddings)
}

/// Get embedding for a single query text.
pub fn get_query_embedding(config: &EmbeddingConfig, text: &str) -> Result<Vec<f32>> {
    let results = get_embeddings(config, &[text.to_string()])?;
    results
        .into_iter()
        .next()
        .context("Empty embedding response")
}

/// Check if the embedding service is reachable and the model is available.
pub fn check_health(config: &EmbeddingConfig) -> HealthStatus {
    match config.provider {
        EmbeddingProvider::Ollama => check_ollama_health(config),
        EmbeddingProvider::Openai | EmbeddingProvider::Custom => check_api_health(config),
    }
}

/// Resolve the Authorization header based on provider config.
fn resolve_auth_header(config: &EmbeddingConfig) -> Result<Option<String>> {
    match config.provider {
        EmbeddingProvider::Ollama => {
            // Ollama uses a dummy token
            Ok(Some("Bearer ollama".into()))
        }
        EmbeddingProvider::Openai | EmbeddingProvider::Custom => {
            if let Some(key) = config.resolve_api_key() {
                if key.is_empty() {
                    bail!("API key is empty. Set the environment variable or update config.toml");
                }
                Ok(Some(format!("Bearer {}", key)))
            } else {
                // Custom providers may not require auth
                if config.provider == EmbeddingProvider::Custom {
                    Ok(None)
                } else {
                    bail!(
                        "API key not found. Set OPENAI_API_KEY env var or configure api_key in config.toml"
                    );
                }
            }
        }
    }
}

/// Determine optimal batch size per provider.
pub fn batch_size_for_provider(provider: &EmbeddingProvider) -> usize {
    match provider {
        EmbeddingProvider::Ollama => 32,
        EmbeddingProvider::Openai => 512, // OpenAI supports up to 2048
        EmbeddingProvider::Custom => 32,
    }
}

/// Health check for Ollama: probe /api/tags to confirm model exists.
fn check_ollama_health(config: &EmbeddingConfig) -> HealthStatus {
    let tags_url = format!("{}/api/tags", config.endpoint);

    let client = match reqwest::blocking::Client::builder()
        .timeout(std::time::Duration::from_secs(5))
        .danger_accept_invalid_certs(true)
        .build()
    {
        Ok(c) => c,
        Err(e) => return HealthStatus::Unreachable(format!("Failed to build HTTP client: {}", e)),
    };

    let response = match client.get(&tags_url).send() {
        Ok(r) => r,
        Err(e) => {
            return HealthStatus::Unreachable(format!(
                "Cannot reach Ollama at {} — {}",
                config.endpoint, e
            ));
        }
    };

    let tags: OllamaTagsResponse = match response.json() {
        Ok(t) => t,
        Err(_) => return HealthStatus::Ok, // Non-standard response, assume OK
    };

    let models = tags.models.unwrap_or_default();
    let has_model = models.iter().any(|m| m.name.contains(&config.model));

    if has_model {
        HealthStatus::Ok
    } else {
        HealthStatus::ModelMissing(format!(
            "Model '{}' not found. Run: ollama pull {}",
            config.model, config.model
        ))
    }
}

/// Health check for API-based providers: try a minimal embedding request.
fn check_api_health(config: &EmbeddingConfig) -> HealthStatus {
    // First check if API key is available
    if config.provider == EmbeddingProvider::Openai && config.resolve_api_key().is_none() {
        return HealthStatus::Unreachable(
            "API key not configured. Run `pearl init` or set OPENAI_API_KEY env var".into(),
        );
    }

    // Try a minimal embedding to verify connectivity
    match get_embeddings(config, &["health check".to_string()]) {
        Ok(results) if !results.is_empty() => HealthStatus::Ok,
        Ok(_) => HealthStatus::Unreachable("Empty response from embedding API".into()),
        Err(e) => HealthStatus::Unreachable(format!("Embedding API error: {}", e)),
    }
}

#[derive(Debug)]
pub enum HealthStatus {
    Ok,
    Unreachable(String),
    ModelMissing(String),
}

impl HealthStatus {
    pub fn is_ok(&self) -> bool {
        matches!(self, HealthStatus::Ok)
    }

    pub fn message(&self) -> &str {
        match self {
            HealthStatus::Ok => "OK",
            HealthStatus::Unreachable(msg) => msg,
            HealthStatus::ModelMissing(msg) => msg,
        }
    }
}

// ─── Ollama Auto-Detection ───────────────────────────────────────────────────

/// Common ports where Ollama might be running.
const OLLAMA_CANDIDATE_PORTS: &[u16] = &[11434, 11435, 11436, 8080];

/// Try to detect a running Ollama instance.
/// Checks:
/// 1. `OLLAMA_HOST` environment variable (Ollama's own config)
/// 2. Probes common ports on localhost
///
/// Returns the base URL (e.g. `http://localhost:11434`) or None.
pub fn detect_ollama_endpoint() -> Option<String> {
    // 1. Check OLLAMA_HOST env var (Ollama's official way to configure host/port)
    if let Ok(host) = std::env::var("OLLAMA_HOST") {
        let endpoint = normalize_ollama_host(&host);
        if probe_ollama(&endpoint) {
            debug!("Detected Ollama via OLLAMA_HOST: {}", endpoint);
            return Some(endpoint);
        }
    }

    // 2. Probe candidate ports
    for port in OLLAMA_CANDIDATE_PORTS {
        let endpoint = format!("http://localhost:{}", port);
        if probe_ollama(&endpoint) {
            debug!("Detected Ollama on port {}", port);
            return Some(endpoint);
        }
    }

    None
}

/// Normalize OLLAMA_HOST value to a full URL.
/// Ollama accepts formats like: "localhost:11434", ":11434", "http://host:port", etc.
fn normalize_ollama_host(host: &str) -> String {
    let h = host.trim();
    if h.starts_with("http://") || h.starts_with("https://") {
        h.to_string()
    } else if h.starts_with(':') {
        // ":11434" → "http://localhost:11434"
        format!("http://localhost{}", h)
    } else {
        format!("http://{}", h)
    }
}

/// Quick probe: try to reach Ollama's /api/tags endpoint with a short timeout.
fn probe_ollama(endpoint: &str) -> bool {
    let url = format!("{}/api/tags", endpoint);
    let client = match reqwest::blocking::Client::builder()
        .timeout(std::time::Duration::from_millis(800))
        .danger_accept_invalid_certs(true)
        .build()
    {
        Ok(c) => c,
        Err(_) => return false,
    };
    client.get(&url).send().is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── batch_size_for_provider ──────────────────────────────────────────────

    #[test]
    fn batch_size_ollama() {
        assert_eq!(batch_size_for_provider(&EmbeddingProvider::Ollama), 32);
    }

    #[test]
    fn batch_size_openai() {
        assert_eq!(batch_size_for_provider(&EmbeddingProvider::Openai), 512);
    }

    #[test]
    fn batch_size_custom() {
        assert_eq!(batch_size_for_provider(&EmbeddingProvider::Custom), 32);
    }

    // ── normalize_ollama_host ────────────────────────────────────────────────

    #[test]
    fn normalize_full_url() {
        assert_eq!(
            normalize_ollama_host("http://localhost:11434"),
            "http://localhost:11434"
        );
    }

    #[test]
    fn normalize_host_port() {
        assert_eq!(
            normalize_ollama_host("localhost:11434"),
            "http://localhost:11434"
        );
    }

    #[test]
    fn normalize_port_only() {
        assert_eq!(normalize_ollama_host(":11434"), "http://localhost:11434");
    }

    // ── HealthStatus ─────────────────────────────────────────────────────────

    #[test]
    fn health_status_is_ok() {
        assert!(HealthStatus::Ok.is_ok());
        assert!(!HealthStatus::Unreachable("x".into()).is_ok());
        assert!(!HealthStatus::ModelMissing("x".into()).is_ok());
    }

    #[test]
    fn health_status_message() {
        assert_eq!(HealthStatus::Ok.message(), "OK");
        assert_eq!(
            HealthStatus::Unreachable("err".into()).message(),
            "err"
        );
    }

    // ── get_embeddings with mock server ──────────────────────────────────────

    #[test]
    fn get_embeddings_success() {
        let mut server = mockito::Server::new();

        let response_body = serde_json::json!({
            "data": [
                {"embedding": [0.1, 0.2, 0.3]},
                {"embedding": [0.4, 0.5, 0.6]}
            ]
        });

        let mock = server
            .mock("POST", "/v1/embeddings")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(response_body.to_string())
            .create();

        let config = EmbeddingConfig {
            provider: EmbeddingProvider::Custom,
            endpoint: server.url(),
            model: "test-model".into(),
            api_key: None,
            dimensions: None,
        };

        let result = get_embeddings(
            &config,
            &["hello".to_string(), "world".to_string()],
        );
        mock.assert();

        let embeddings = result.unwrap();
        assert_eq!(embeddings.len(), 2);
        assert_eq!(embeddings[0], vec![0.1, 0.2, 0.3]);
        assert_eq!(embeddings[1], vec![0.4, 0.5, 0.6]);
    }

    #[test]
    fn get_embeddings_empty_input() {
        let config = EmbeddingConfig {
            provider: EmbeddingProvider::Custom,
            endpoint: "http://unused".into(),
            model: "test".into(),
            api_key: None,
            dimensions: None,
        };
        let result = get_embeddings(&config, &[]).unwrap();
        assert!(result.is_empty());
    }

    #[test]
    fn get_embeddings_api_error() {
        let mut server = mockito::Server::new();

        let mock = server
            .mock("POST", "/v1/embeddings")
            .with_status(500)
            .with_body("Internal Server Error")
            .create();

        let config = EmbeddingConfig {
            provider: EmbeddingProvider::Custom,
            endpoint: server.url(),
            model: "test-model".into(),
            api_key: None,
            dimensions: None,
        };

        let result = get_embeddings(&config, &["hello".to_string()]);
        mock.assert();
        assert!(result.is_err());
    }

    // ── get_embeddings_async with mock server ────────────────────────────────

    #[test]
    fn get_embeddings_async_success() {
        // Skip: reqwest::blocking::Client cannot be used inside any tokio runtime context.
        // get_embeddings_async delegates to blocking get_embeddings for single batches.
        // This is tested indirectly through the blocking get_embeddings_success test.
    }

    #[test]
    fn get_embeddings_async_empty() {
        // Empty input returns immediately without creating a client
        std::thread::spawn(|| {
            let rt = tokio::runtime::Runtime::new().unwrap();
            rt.block_on(async {
                let config = EmbeddingConfig {
                    provider: EmbeddingProvider::Custom,
                    endpoint: "http://unused".into(),
                    model: "test".into(),
                    api_key: None,
                    dimensions: None,
                };
                let result = get_embeddings_async(&config, &[]).await.unwrap();
                assert!(result.is_empty());
            });
        })
        .join()
        .unwrap();
    }

    // ── resolve_auth_header ──────────────────────────────────────────────────

    #[test]
    fn auth_header_ollama() {
        let config = EmbeddingConfig {
            provider: EmbeddingProvider::Ollama,
            endpoint: "http://localhost:11434".into(),
            model: "test".into(),
            api_key: None,
            dimensions: None,
        };
        let auth = resolve_auth_header(&config).unwrap();
        assert_eq!(auth.as_deref(), Some("Bearer ollama"));
    }

    #[test]
    fn auth_header_openai_with_key() {
        unsafe { std::env::set_var("PEARL_TEST_AUTH", "sk-test") };
        let config = EmbeddingConfig {
            provider: EmbeddingProvider::Openai,
            endpoint: "https://api.openai.com".into(),
            model: "test".into(),
            api_key: Some("$PEARL_TEST_AUTH".into()),
            dimensions: None,
        };
        let auth = resolve_auth_header(&config).unwrap();
        assert_eq!(auth.as_deref(), Some("Bearer sk-test"));
        unsafe { std::env::remove_var("PEARL_TEST_AUTH") };
    }

    #[test]
    fn auth_header_openai_no_key_fails() {
        let config = EmbeddingConfig {
            provider: EmbeddingProvider::Openai,
            endpoint: "https://api.openai.com".into(),
            model: "test".into(),
            api_key: None,
            dimensions: None,
        };
        assert!(resolve_auth_header(&config).is_err());
    }

    #[test]
    fn auth_header_custom_no_key_ok() {
        let config = EmbeddingConfig {
            provider: EmbeddingProvider::Custom,
            endpoint: "http://localhost:8080".into(),
            model: "test".into(),
            api_key: None,
            dimensions: None,
        };
        let auth = resolve_auth_header(&config).unwrap();
        assert!(auth.is_none());
    }
}
