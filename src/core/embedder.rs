use anyhow::{bail, Context, Result};
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

/// Get embeddings for a batch of texts.
pub fn get_embeddings(config: &EmbeddingConfig, texts: &[String]) -> Result<Vec<Vec<f32>>> {
    if texts.is_empty() {
        return Ok(vec![]);
    }

    let url = format!("{}/v1/embeddings", config.endpoint);
    let auth_header = resolve_auth_header(config)?;
    let batch_size = batch_size_for_provider(&config.provider);
    let mut all_embeddings = Vec::new();

    for chunk in texts.chunks(batch_size) {
        let request = EmbeddingRequest {
            model: config.model.clone(),
            input: chunk.to_vec(),
            dimensions: config.dimensions,
        };

        let mut req = ureq::post(&url).header("Content-Type", "application/json");

        if let Some(ref auth) = auth_header {
            req = req.header("Authorization", auth);
        }

        let response: EmbeddingResponse = req
            .send_json(&request)
            .context("Failed to call embedding API")?
            .body_mut()
            .read_json()
            .context("Failed to parse embedding response")?;

        for data in response.data {
            all_embeddings.push(data.embedding);
        }
    }

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
                    bail!(
                        "API key is empty. Set the environment variable or update config.toml"
                    );
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
fn batch_size_for_provider(provider: &EmbeddingProvider) -> usize {
    match provider {
        EmbeddingProvider::Ollama => 32,
        EmbeddingProvider::Openai => 100, // OpenAI supports up to 2048
        EmbeddingProvider::Custom => 32,
    }
}

/// Health check for Ollama: probe /api/tags to confirm model exists.
fn check_ollama_health(config: &EmbeddingConfig) -> HealthStatus {
    let tags_url = format!("{}/api/tags", config.endpoint);

    let mut response = match ureq::get(&tags_url).call() {
        Ok(r) => r,
        Err(e) => {
            return HealthStatus::Unreachable(format!(
                "Cannot reach Ollama at {} — {}",
                config.endpoint, e
            ));
        }
    };

    let tags: OllamaTagsResponse = match response.body_mut().read_json() {
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
            "API key not configured. Run `vault-search-mcp init` or set OPENAI_API_KEY env var".into(),
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
    let agent = ureq::Agent::config_builder()
        .timeout_global(Some(std::time::Duration::from_millis(800)))
        .build()
        .new_agent();
    agent.get(&url).call().is_ok()
}
