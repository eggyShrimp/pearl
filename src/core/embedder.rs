use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::config::EmbeddingConfig;

#[derive(Serialize)]
struct EmbeddingRequest {
    model: String,
    input: Vec<String>,
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
    let mut all_embeddings = Vec::new();

    // Batch in groups of 32
    for chunk in texts.chunks(32) {
        let request = EmbeddingRequest {
            model: config.model.clone(),
            input: chunk.to_vec(),
        };

        let response: EmbeddingResponse = ureq::post(&url)
            .header("Content-Type", "application/json")
            .header("Authorization", "Bearer ollama")
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

/// Check if Ollama is reachable and the model is available.
pub fn check_health(config: &EmbeddingConfig) -> HealthStatus {
    let tags_url = format!("{}/api/tags", config.endpoint);

    let mut response = match ureq::get(&tags_url).call() {
        Ok(r) => r,
        Err(e) => {
            return HealthStatus::Unreachable(format!(
                "Cannot reach {} — {}",
                config.endpoint, e
            ));
        }
    };

    let tags: OllamaTagsResponse = match response.body_mut().read_json() {
        Ok(t) => t,
        Err(_) => return HealthStatus::Ok, // Non-Ollama endpoint, assume OK
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
