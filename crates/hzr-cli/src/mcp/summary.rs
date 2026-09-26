//! 0.11.2: the text block that accompanies `structuredContent`.
//!
//! Every successful tool result carried its complete payload twice — once as
//! `structuredContent` and once re-serialized into the text block — so a 28 KB `hzr_read`
//! crossed the wire as 62 KB. The MCP revisions that define `structuredContent`
//! (2025-06-18 and later) let the text block be a compact rendering instead, and the
//! clients that read `structuredContent` (Claude Code, Codex) hand the model that field,
//! not the text. The compact view keeps every small field — line ranges, hashes,
//! `next_line`, `remaining_paths`, `next_step`, cursors, `output_omitted` — so bounded and
//! recovery information stays visible in both blocks; only large strings and long arrays
//! are replaced by a pointer to the structured field.

use serde_json::{Map, Value};

/// Longest string copied verbatim into the text block, in characters.
const INLINE_STRING_CHARS: usize = 120;
/// Array elements copied into the text block before the rest is pointed at.
const INLINE_ARRAY_ITEMS: usize = 10;

/// Compact text for a result whose complete payload travels in `structuredContent`.
pub(super) fn compact_text(tool: &str, value: &Value) -> String {
    format!(
        "{tool} result (compact view; the complete payload is in structuredContent): {}",
        compact_value(value)
    )
}

fn compact_value(value: &Value) -> Value {
    match value {
        Value::String(text) => match text.char_indices().nth(INLINE_STRING_CHARS) {
            None => value.clone(),
            Some(_) => Value::String(format!(
                "<{} bytes, {} lines: see structuredContent>",
                text.len(),
                text.lines().count()
            )),
        },
        Value::Array(items) => {
            let mut compact = items
                .iter()
                .take(INLINE_ARRAY_ITEMS)
                .map(compact_value)
                .collect::<Vec<_>>();
            if items.len() > INLINE_ARRAY_ITEMS {
                compact.push(Value::String(format!(
                    "<{} more items: see structuredContent>",
                    items.len() - INLINE_ARRAY_ITEMS
                )));
            }
            Value::Array(compact)
        }
        Value::Object(fields) => Value::Object(
            fields
                .iter()
                .map(|(key, field)| (key.clone(), compact_value(field)))
                .collect::<Map<_, _>>(),
        ),
        Value::Null | Value::Bool(_) | Value::Number(_) => value.clone(),
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::compact_text;

    #[test]
    fn compact_text_keeps_bounds_and_recovery_but_not_the_payload() {
        let content = "line of source text\n".repeat(1_400);
        let value = json!({
            "files": [{
                "path": "src/lib.rs", "source_sha256": "a".repeat(64), "total_lines": 1_400,
                "from": 1, "to": 1_400, "next_line": 1_401, "complete": false,
                "content": content
            }],
            "remaining_paths": ["src/main.rs"], "estimated_tokens": 7_000, "max_tokens": 8_192
        });

        let text = compact_text("hzr_read", &value);

        assert!(text.len() < 600, "{} bytes: {text}", text.len());
        for kept in [
            "\"next_line\":1401",
            "\"complete\":false",
            "\"remaining_paths\":[\"src/main.rs\"]",
            "\"max_tokens\":8192",
            "structuredContent",
        ] {
            assert!(text.contains(kept), "missing {kept}: {text}");
        }
        assert!(text.contains(&format!("<{} bytes, 1400 lines", content.len())));
        assert!(!text.contains("line of source text"));
    }

    #[test]
    fn compact_text_bounds_long_arrays() {
        let value = json!({"hits": (0..25).map(|index| json!({"path": format!("f{index}")})).collect::<Vec<_>>()});
        let text = compact_text("hzr_search", &value);
        assert!(text.contains("\"f9\""));
        assert!(!text.contains("\"f10\""));
        assert!(text.contains("<15 more items: see structuredContent>"));
    }
}
