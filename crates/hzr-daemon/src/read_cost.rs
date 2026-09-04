use std::{collections::HashMap, sync::Arc, time::Instant};

use hzr_protocol::{ReadCostAdvice, ReadFileResult};
use sha2::{Digest, Sha256};
use tokio::sync::Mutex;

pub(crate) const ADVICE_RESERVE_BYTES: u64 = 512;
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

/// Exact JSON string content bytes without allocation or per-line serialization.
pub(crate) fn source_json_bytes(source: &str) -> u64 {
    source
        .bytes()
        .map(|byte| match byte {
            b'"' | b'\\' | b'\n' | b'\r' | b'\t' | 8 | 12 => 2,
            0..=31 => 6,
            _ => 1,
        })
        .sum()
}

fn advice_costs(advice: &mut ReadCostAdvice, selected: u64, full: u64, envelope: u64, prior: u64) {
    // Both alternatives include the same serialized advice and allocated envelope.
    // Counter digit widths converge monotonically; no source is serialized in this loop.
    advice.produced_tokens_estimated = 0;
    advice.full_result_tokens_estimated = 0;
    loop {
        let overhead = serde_json::to_vec(advice)
            .expect("scalar read advice is serializable")
            .len() as u64
            + "\"cost_advice\":,".len() as u64
            + envelope;
        let produced = prior.saturating_add(selected.saturating_add(overhead).div_ceil(4));
        let full = full.saturating_add(overhead).div_ceil(4);
        if produced == advice.produced_tokens_estimated
            && full == advice.full_result_tokens_estimated
        {
            break;
        }
        advice.produced_tokens_estimated = produced;
        advice.full_result_tokens_estimated = full;
    }
}

impl ReadCosts {
    /// Call only for a successfully finalized response; a failed batch has no coverage.
    pub(crate) async fn observe(
        &self,
        identity: [&str; 5],
        file: &ReadFileResult,
        full_source_json_bytes: u64,
        envelope_bytes: u64,
    ) -> ReadCostAdvice {
        let selected_bytes = serde_json::to_vec(file)
            .expect("read result is serializable")
            .len() as u64;
        let mut full_file = file.clone();
        full_file.from = 1;
        full_file.to = file.total_lines;
        full_file.next_line = None;
        full_file.complete = true;
        full_file.content.clear();
        full_file.cost_advice = None;
        let full_bytes = (serde_json::to_vec(&full_file)
            .expect("read result is serializable")
            .len() as u64)
            .saturating_add(full_source_json_bytes);

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
        // Inspect only returned bytes, with one forward pass through sorted coverage.
        let mut span_index = 0;
        let mut repeated_bytes = 0u64;
        for (index, line) in file.content.split_inclusive('\n').enumerate() {
            let number = file.from.saturating_add(index as u64);
            while span_index < episode.spans.len() && episode.spans[span_index].1 < number {
                span_index += 1;
            }
            if episode
                .spans
                .get(span_index)
                .is_some_and(|(from, to)| number >= *from && number <= *to)
            {
                repeated_bytes = repeated_bytes.saturating_add(line.len() as u64);
            }
        }
        episode.requests = episode.requests.saturating_add(1);
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
        let next_missing_to = next_missing_from.map(|missing| {
            episode
                .spans
                .iter()
                .find(|(from, _)| *from > missing)
                .map_or(file.total_lines, |(from, _)| from - 1)
        });
        let mut advice = ReadCostAdvice {
            method: "produced_utf8_bytes_div_4_advisory_v2".into(),
            requests: episode.requests,
            produced_tokens_estimated: 0,
            repeated_source_tokens_estimated: episode.repeated,
            full_result_tokens_estimated: 0,
            next_action: if next_missing_from.is_none() {
                "complete"
            } else {
                "read_remaining"
            }
            .into(),
            next_missing_from,
            next_missing_to,
        };
        advice_costs(
            &mut advice,
            selected_bytes,
            full_bytes,
            envelope_bytes,
            episode.produced,
        );
        if next_missing_from.is_some()
            && (episode.requests <= 1
                || advice.produced_tokens_estimated < advice.full_result_tokens_estimated)
        {
            advice.next_action = "continue_if_needed".into();
            advice_costs(
                &mut advice,
                selected_bytes,
                full_bytes,
                envelope_bytes,
                episode.produced,
            );
        }
        episode.produced = advice.produced_tokens_estimated;
        advice
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn file(from: u64, to: u64) -> ReadFileResult {
        let lines = ["one\n", "two\n", "end\n"];
        ReadFileResult {
            path: "source.rs".into(),
            source_sha256: "a".repeat(64),
            source_bytes: 12,
            total_lines: 3,
            from,
            to,
            next_line: Some(to + 1),
            complete: false,
            content: lines[(from - 1) as usize..to as usize].concat(),
            cost_advice: None,
        }
    }

    #[test]
    fn scalar_advice_always_fits_the_reserved_response_budget() {
        let advice = ReadCostAdvice {
            method: "produced_utf8_bytes_div_4_advisory_v2".into(),
            requests: u64::MAX,
            produced_tokens_estimated: u64::MAX,
            repeated_source_tokens_estimated: u64::MAX,
            full_result_tokens_estimated: u64::MAX,
            next_action: "continue_if_needed".into(),
            next_missing_from: Some(u64::MAX),
            next_missing_to: Some(u64::MAX),
        };
        assert!(
            serde_json::to_vec(&advice).expect("JSON").len() as u64
                + "\"cost_advice\":,".len() as u64
                <= ADVICE_RESERVE_BYTES
        );
    }

    #[test]
    fn source_byte_counter_matches_json_escaping_without_line_allocations() {
        for source in [
            "",
            "one\r\ntwo",
            "λ😀\t\"\\\u{0000}\u{0008}\u{000c}",
            "\n\n\n",
        ] {
            assert_eq!(
                source_json_bytes(source),
                serde_json::to_vec(source).expect("JSON").len() as u64 - 2
            );
        }
    }

    #[tokio::test]
    async fn repeats_trigger_remaining_evidence_without_suppressing_content() {
        let costs = ReadCosts::default();
        let identity = ["/repo", "source.rs", "session", "epoch", "agent"];
        costs.observe(identity, &file(1, 1), 15, 80).await;
        let repeated = costs.observe(identity, &file(1, 1), 15, 80).await;
        assert_eq!(repeated.repeated_source_tokens_estimated, 1);
        assert_eq!(repeated.next_action, "read_remaining");
        assert_eq!(repeated.next_missing_from, Some(2));
        let completed = costs.observe(identity, &file(2, 3), 15, 80).await;
        assert_eq!(completed.next_action, "complete");
    }

    #[tokio::test]
    async fn epoch_workspace_and_source_hash_reset_the_advisory() {
        let costs = ReadCosts::default();
        costs
            .observe(["/a", "f", "s", "old", "agent"], &file(1, 1), 15, 80)
            .await;
        for identity in [
            ["/a", "f", "s", "new", "agent"],
            ["/b", "f", "s", "old", "agent"],
        ] {
            assert_eq!(
                costs.observe(identity, &file(1, 1), 15, 80).await.requests,
                1
            );
        }
        let mut changed = file(1, 1);
        changed.source_sha256 = "b".repeat(64);
        assert_eq!(
            costs
                .observe(["/a", "f", "s", "old", "agent"], &changed, 15, 80)
                .await
                .requests,
            1
        );
    }

    #[tokio::test]
    async fn advice_and_envelope_cost_trigger_crossover_after_three_ranges() {
        let costs = ReadCosts::default();
        let source = "abcdefgh\n".repeat(100);
        let mut range = file(1, 1);
        range.source_bytes = source.len() as u64;
        range.total_lines = 100;
        range.content = "abcdefgh\n".into();
        let mut measured = 0u64;
        for from in 1..=3 {
            range.from = from;
            range.to = from;
            range.next_line = Some(from + 1);
            let advice = costs
                .observe(
                    ["/repo", "f", "s", "epoch", "agent"],
                    &range,
                    source_json_bytes(&source),
                    80,
                )
                .await;
            range.cost_advice = Some(advice.clone());
            measured += (serde_json::to_vec(&range).expect("wire").len() as u64 + 80).div_ceil(4);
            assert_eq!(advice.produced_tokens_estimated, measured);
            assert_eq!(
                advice.next_action,
                if from == 3 {
                    "read_remaining"
                } else {
                    "continue_if_needed"
                }
            );
            range.cost_advice = None;
        }
    }
}
