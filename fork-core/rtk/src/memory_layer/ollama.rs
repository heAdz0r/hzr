//! E9.2: Ollama local ML adapter — intent classify + top-K rerank (ADR-0003).
//! Uses stdlib TcpStream (no external HTTP deps). JSON-only protocol.
//! Strict timeout + graceful fallback: returns None on any error.

use anyhow::{bail, Result};
use serde::{Deserialize, Serialize};
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::time::Duration;

use super::intent::IntentKind;

// ── Config ─────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct OllamaConfig {
    /// e.g. "localhost:11434"
    pub host: String,
    pub timeout_ms: u64,
    /// Model for top-K reranking (e.g. "nomic-embed-text", "llama3.2")
    pub rerank_model: String,
    /// Model for intent classification (e.g. "llama3.2")
    pub classify_model: String,
}

impl Default for OllamaConfig {
    fn default() -> Self {
        Self {
            host: "localhost:11434".to_string(),
            timeout_ms: 3000,
            rerank_model: "llama3.2".to_string(),
            classify_model: "llama3.2".to_string(),
        }
    }
}

// ── HTTP primitive ─────────────────────────────────────────────────────────────

/// Minimal sync HTTP/1.1 POST using stdlib TcpStream.
/// Returns raw JSON response body on success.
fn http_post(host: &str, path: &str, body: &str, timeout_ms: u64) -> Result<String> {
    let timeout = Duration::from_millis(timeout_ms);
    let addr: SocketAddr = host
        .parse()
        .map_err(|_| anyhow::anyhow!("Invalid Ollama host: {}", host))?;
    let mut stream = TcpStream::connect_timeout(&addr, timeout)?;
    stream.set_read_timeout(Some(timeout))?;
    stream.set_write_timeout(Some(timeout))?;

    let request = format!(
        "POST {path} HTTP/1.1\r\nHost: {host}\r\nContent-Type: application/json\r\nContent-Length: {len}\r\nConnection: close\r\n\r\n{body}",
        len = body.len(),
    );
    stream.write_all(request.as_bytes())?;

    let mut response = String::new();
    stream.read_to_string(&mut response)?;

    // Extract body after \r\n\r\n header separator
    if let Some(pos) = response.find("\r\n\r\n") {
        Ok(response[pos + 4..].to_string())
    } else {
        bail!("Malformed HTTP response from Ollama (no header separator)")
    }
}

// ── Rerank ─────────────────────────────────────────────────────────────────────

/// Rerank top-K candidate file paths using Ollama.
/// Returns scores in [0.0, 1.0] per candidate (same order as input).
/// Returns `None` on timeout or any Ollama error — caller falls back to Stage-1.
pub fn rerank_candidates(
    candidates: &[String], // rel_paths (top-K already selected by Stage-1)
    task: &str,
    config: &OllamaConfig,
) -> Option<Vec<f32>> {
    if candidates.is_empty() {
        return Some(Vec::new());
    }
    let prompt = build_rerank_prompt(candidates, task);
    let payload = serde_json::json!({
        "model": config.rerank_model,
        "prompt": prompt,
        "stream": false,
        "format": "json",
    });
    let body = serde_json::to_string(&payload).ok()?;
    let raw = http_post(&config.host, "/api/generate", &body, config.timeout_ms).ok()?;

    #[derive(Deserialize)]
    struct GenerateResp {
        response: Option<String>,
    }
    let outer: GenerateResp = serde_json::from_str(&raw).ok()?;
    parse_rerank_scores(&outer.response?, candidates.len())
}

fn build_rerank_prompt(candidates: &[String], task: &str) -> String {
    let list = candidates
        .iter()
        .enumerate()
        .map(|(i, p)| format!("{i}. {p}"))
        .collect::<Vec<_>>()
        .join("\n");
    format!(
        "You are a code context relevance ranker.\n\
         Task: {task}\n\
         Files:\n{list}\n\n\
         Score each file 0.0-1.0 for relevance to the task.\n\
         Respond ONLY with valid JSON: {{\"scores\": [0.8, 0.3, ...]}} in the same order as the files."
    )
}

fn parse_rerank_scores(json: &str, expected: usize) -> Option<Vec<f32>> {
    #[derive(Deserialize)]
    struct ScoreResp {
        scores: Vec<f32>,
    }
    let resp: ScoreResp = serde_json::from_str(json).ok()?;
    if resp.scores.len() != expected {
        return None; // schema mismatch — fallback to Stage-1
    }
    Some(resp.scores.iter().map(|&s| s.clamp(0.0, 1.0)).collect())
}

// ── Intent classify ────────────────────────────────────────────────────────────

/// Optional intent classification via Ollama. Augments the Rust rule-based classifier.
/// Returns `None` on any error — caller falls back to Rust rules.
pub fn classify_intent_ollama(task: &str, config: &OllamaConfig) -> Option<(IntentKind, f32)> {
    let prompt = format!(
        "Classify this software task into exactly one intent: bugfix, feature, refactor, incident, or unknown.\n\
         Task: {task}\n\
         Respond ONLY with valid JSON: {{\"intent\": \"bugfix\", \"confidence\": 0.9}}"
    );
    let payload = serde_json::json!({
        "model": config.classify_model,
        "prompt": prompt,
        "stream": false,
        "format": "json",
    });
    let body = serde_json::to_string(&payload).ok()?;
    let raw = http_post(&config.host, "/api/generate", &body, config.timeout_ms).ok()?;

    #[derive(Deserialize)]
    struct GenerateResp {
        response: Option<String>,
    }
    #[derive(Deserialize)]
    struct ClassifyResp {
        intent: Option<String>,
        confidence: Option<f32>,
    }

    let outer: GenerateResp = serde_json::from_str(&raw).ok()?;
    let inner: ClassifyResp = serde_json::from_str(&outer.response?).ok()?;

    let kind = match inner.intent?.as_str() {
        "bugfix" => IntentKind::Bugfix,
        "feature" => IntentKind::Feature,
        "refactor" => IntentKind::Refactor,
        "incident" => IntentKind::Incident,
        _ => IntentKind::Unknown,
    };
    Some((kind, inner.confidence.unwrap_or(0.5).clamp(0.0, 1.0)))
}

// ── Model status ───────────────────────────────────────────────────────────────

#[derive(Debug, Serialize, Deserialize)]
pub struct OllamaStatus {
    pub reachable: bool,
    pub host: String,
    pub latency_ms: Option<u64>,
}

/// Probe Ollama availability by hitting /api/tags. Returns status without error.
pub fn probe_ollama(config: &OllamaConfig) -> OllamaStatus {
    let t = std::time::Instant::now();
    let reachable = http_post(&config.host, "/api/tags", "{}", config.timeout_ms).is_ok();
    OllamaStatus {
        reachable,
        host: config.host.clone(),
        latency_ms: if reachable {
            Some(t.elapsed().as_millis() as u64)
        } else {
            None
        },
    }
}

// ── Tests ──────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn dead_config() -> OllamaConfig {
        OllamaConfig {
            host: "127.0.0.1:19999".to_string(), // not running
            timeout_ms: 100,
            ..Default::default()
        }
    }

    #[test]
    fn test_parse_rerank_scores_valid() {
        let json = r#"{"scores": [0.9, 0.5, 0.1]}"#;
        let scores = parse_rerank_scores(json, 3).unwrap();
        assert_eq!(scores, vec![0.9, 0.5, 0.1]);
    }

    #[test]
    fn test_parse_rerank_scores_clamps() {
        let json = r#"{"scores": [1.5, -0.3, 0.5]}"#;
        let scores = parse_rerank_scores(json, 3).unwrap();
        assert_eq!(scores[0], 1.0, "should clamp to 1.0");
        assert_eq!(scores[1], 0.0, "should clamp to 0.0");
        assert_eq!(scores[2], 0.5);
    }

    #[test]
    fn test_parse_rerank_scores_wrong_count_returns_none() {
        let json = r#"{"scores": [0.9, 0.5]}"#;
        assert!(parse_rerank_scores(json, 3).is_none());
    }

    #[test]
    fn test_rerank_unavailable_returns_none() {
        let result = rerank_candidates(
            &["a.rs".to_string(), "b.rs".to_string()],
            "fix bug",
            &dead_config(),
        );
        assert!(
            result.is_none(),
            "should gracefully return None when Ollama is down"
        );
    }

    #[test]
    fn test_classify_intent_unavailable_returns_none() {
        let result = classify_intent_ollama("fix the auth bug", &dead_config());
        assert!(
            result.is_none(),
            "should gracefully return None when Ollama is down"
        );
    }

    #[test]
    fn test_probe_ollama_returns_unreachable() {
        let status = probe_ollama(&dead_config());
        assert!(!status.reachable);
        assert!(status.latency_ms.is_none());
    }

    #[test]
    fn test_empty_candidates_rerank() {
        let result = rerank_candidates(&[], "fix bug", &dead_config());
        // Empty candidates short-circuits before hitting Ollama
        assert_eq!(result, Some(vec![]));
    }
}
