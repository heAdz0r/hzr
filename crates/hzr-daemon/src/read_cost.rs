use std::{collections::HashMap, sync::Arc, time::Instant};

use hzr_protocol::{ReadCostAdvice, ReadFileResult};
use sha2::{Digest, Sha256};
use tokio::sync::Mutex;

const MAX_EPISODES: usize = 1024;
const MAX_SPANS: usize = 128;

#[derive(Clone, Default)]
pub(crate) struct ReadCosts(Arc<Mutex<HashMap<[u8; 32], Episode>>>);

struct Episode {
    touched: Instant,
    requests: u64,
    produced: u64,
    repeated: u64,
    spans: Vec<(u64, u64)>,
}

impl ReadCosts {
    pub(crate) async fn observe(
        &self,
        identity: [&str; 5],
        file: &ReadFileResult,
        lines: &[&str],
    ) -> ReadCostAdvice {
        let mut hash = Sha256::new();
        for part in identity {
            hash.update((part.len() as u64).to_le_bytes());
            hash.update(part.as_bytes());
        }
        hash.update(file.source_sha256.as_bytes());
        let key: [u8; 32] = hash.finalize().into();
        let mut episodes = self.0.lock().await;
        if !episodes.contains_key(&key) && episodes.len() >= MAX_EPISODES {
            if let Some(oldest) = episodes
                .iter()
                .min_by_key(|(_, value)| value.touched)
                .map(|(key, _)| *key)
            {
                episodes.remove(&oldest);
            }
        }
        let episode = episodes.entry(key).or_insert_with(|| Episode {
            touched: Instant::now(),
            requests: 0,
            produced: 0,
            repeated: 0,
            spans: Vec::new(),
        });
        let repeated_bytes: u64 = lines
            .iter()
            .enumerate()
            .filter(|(index, _)| {
                let line = *index as u64 + 1;
                line >= file.from
                    && line <= file.to
                    && episode
                        .spans
                        .iter()
                        .any(|(from, to)| line >= *from && line <= *to)
            })
            .map(|(_, line)| line.len() as u64)
            .sum();
        // Includes this file's metadata and escaping. This measures produced JSON,
        // not provider tokens or proof that a host retained the response.
        let produced =
            serde_json::to_vec(file).map_or(file.content.len(), |bytes| bytes.len()) as u64;
        episode.requests = episode.requests.saturating_add(1);
        episode.produced = episode.produced.saturating_add(produced.div_ceil(4));
        episode.repeated = episode.repeated.saturating_add(repeated_bytes.div_ceil(4));
        episode.touched = Instant::now();
        if file.from <= file.to {
            episode.spans.push((file.from, file.to));
            episode.spans.sort_unstable();
            let mut merged: Vec<(u64, u64)> = Vec::with_capacity(episode.spans.len());
            for (from, to) in episode.spans.drain(..) {
                if let Some(last) = merged
                    .last_mut()
                    .filter(|last| from <= last.1.saturating_add(1))
                {
                    last.1 = last.1.max(to);
                } else {
                    merged.push((from, to));
                }
            }
            // Forget excess history rather than inventing coverage between gaps.
            merged.truncate(MAX_SPANS);
            episode.spans = merged;
        }
        let mut missing_from = 1;
        for (from, to) in &episode.spans {
            if *from > missing_from {
                break;
            }
            missing_from = missing_from.max(to.saturating_add(1));
        }
        let next_missing_from = (missing_from <= file.total_lines).then_some(missing_from);
        let mut full_file = file.clone();
        full_file.from = 1;
        full_file.to = file.total_lines;
        full_file.next_line = None;
        full_file.complete = true;
        full_file.content.clear();
        full_file.cost_advice = None;
        let metadata_bytes = serde_json::to_vec(&full_file).map_or(0, |bytes| bytes.len()) as u64;
        let source_json_bytes: u64 = lines
            .iter()
            .map(|line| {
                serde_json::to_vec(line).map_or(line.len(), |bytes| bytes.len().saturating_sub(2))
                    as u64
            })
            .sum();
        let full = metadata_bytes.saturating_add(source_json_bytes).div_ceil(4);
        let next_missing_to = next_missing_from.map(|missing| {
            episode
                .spans
                .iter()
                .find(|(from, _)| *from > missing)
                .map_or(file.total_lines, |(from, _)| from - 1)
        });
        let action = if next_missing_from.is_none() {
            "complete"
        } else if episode.requests > 1 && episode.produced >= full {
            "read_remaining"
        } else {
            "continue_if_needed"
        };
        ReadCostAdvice {
            method: "produced_utf8_bytes_div_4_advisory_v1".into(),
            requests: episode.requests,
            produced_tokens_estimated: episode.produced,
            repeated_source_tokens_estimated: episode.repeated,
            full_result_tokens_estimated: full,
            next_action: action.into(),
            next_missing_from,
            next_missing_to,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn file(from: u64, to: u64) -> ReadFileResult {
        ReadFileResult {
            path: "source.rs".into(),
            source_sha256: "a".repeat(64),
            source_bytes: 12,
            total_lines: 3,
            from,
            to,
            next_line: Some(to + 1),
            complete: false,
            content: "one\n".into(),
            cost_advice: None,
        }
    }

    #[tokio::test]
    async fn repeats_trigger_remaining_evidence_without_suppressing_content() {
        let costs = ReadCosts::default();
        let lines = ["one\n", "two\n", "end\n"];
        let identity = ["/repo", "source.rs", "session", "epoch", "agent"];
        costs.observe(identity, &file(1, 1), &lines).await;
        let repeated = costs.observe(identity, &file(1, 1), &lines).await;
        assert_eq!(repeated.repeated_source_tokens_estimated, 1);
        assert_eq!(repeated.next_action, "read_remaining");
        assert_eq!(repeated.next_missing_from, Some(2));
        let completed = costs.observe(identity, &file(2, 3), &lines).await;
        assert_eq!(completed.next_action, "complete");
    }

    #[tokio::test]
    async fn epoch_workspace_and_source_hash_reset_the_advisory() {
        let costs = ReadCosts::default();
        let lines = ["one\n", "two\n", "end\n"];
        costs
            .observe(["/a", "f", "s", "old", "agent"], &file(1, 1), &lines)
            .await;
        for identity in [
            ["/a", "f", "s", "new", "agent"],
            ["/b", "f", "s", "old", "agent"],
        ] {
            assert_eq!(
                costs.observe(identity, &file(1, 1), &lines).await.requests,
                1
            );
        }
        let mut changed = file(1, 1);
        changed.source_sha256 = "b".repeat(64);
        assert_eq!(
            costs
                .observe(["/a", "f", "s", "old", "agent"], &changed, &lines)
                .await
                .requests,
            1
        );
    }
}
