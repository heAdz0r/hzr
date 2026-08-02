use serde_json::{Value, json};

pub(super) fn tool_definitions() -> Vec<Value> {
    vec![
        json!({
            "name": "hzr_memory_recall",
            "title": "Recall HZR Memory",
            "description": "Recall durable decisions, resolved errors and preferences before \
        re-reading prior work. Defaults to this repository plus explicitly user-global memory; \
        memories from another repository are never returned.",
            "inputSchema": {
                "$schema": "https://json-schema.org/draft/2020-12/schema",
                "type": "object",
                "additionalProperties": false,
                "properties": {
                    "query": {
                        "type": "string",
                        "minLength": 1,
                        "description": "Natural-language description of the decision, fact or prior context to recall.",
                    },
                    "topic": {
                        "type": "string",
                        "minLength": 1,
                        "maxLength": 64,
                        "pattern": "^[a-z0-9]+(?:-[a-z0-9]+)*$",
                        "description": "Optional exact memory kind such as architecture, preference or resolved-error.",
                    },
                    "keyword": {
                        "type": "string",
                        "minLength": 1,
                        "description": "Optional keyword filter applied by the memory engine.",
                    },
                    "limit": {
                        "type": "integer",
                        "minimum": 1,
                        "maximum": 50,
                        "default": 10,
                        "description": "Maximum memories returned.",
                    },
                    "scope": {
                        "type": "string",
                        "enum": ["project", "global", "project_and_global"],
                        "default": "project_and_global",
                        "description": "project_and_global returns this repository plus explicit user-wide memory. Use project or global to restrict the lookup.",
                    },
                },
                "required": ["query"],
            },
            "outputSchema": {
                "$schema": "https://json-schema.org/draft/2020-12/schema",
                "type": "object",
                "properties": {
                    "count": {"type": "integer", "minimum": 0},
                    "memories": {"type": "array", "items": {"type": "object"}},
                },
                "required": ["count", "memories"],
            },
            "annotations": {
                "readOnlyHint": true,
                "destructiveHint": false,
                "openWorldHint": false,
            },
        }),
        json!({
            "name": "hzr_memory_store",
            "title": "Store HZR Memory",
            "description": "Persist one durable decision, preference, resolved error or completed \
        handoff in the single HZR-owned store. Do not store ephemeral progress, raw tool output, \
        credentials or speculative conclusions.",
            "inputSchema": {
                "$schema": "https://json-schema.org/draft/2020-12/schema",
                "type": "object",
                "additionalProperties": false,
                "properties": {
                    "topic": {
                        "type": "string",
                        "minLength": 1,
                        "maxLength": 64,
                        "pattern": "^[a-z0-9]+(?:-[a-z0-9]+)*$",
                        "description": "Stable memory kind such as architecture, preference or resolved-error.",
                    },
                    "content": {
                        "type": "string",
                        "minLength": 1,
                        "description": "Self-contained durable fact including the decision and why it matters.",
                    },
                    "importance": {
                        "type": "string",
                        "enum": ["critical", "high", "medium", "low"],
                        "default": "medium",
                        "description": "Retrieval priority. Reserve critical for invariants whose omission can cause harm.",
                    },
                    "keywords": {
                        "type": "array",
                        "maxItems": 32,
                        "items": {"type": "string", "minLength": 1},
                        "description": "Optional exact retrieval terms; at most 32.",
                    },
                    "scope": {
                        "type": "string",
                        "enum": ["project", "global"],
                        "default": "project",
                        "description": "Use global only for a user-wide preference or rule; project is the safe default for repository facts.",
                    },
                },
                "required": ["topic", "content"],
            },
            "outputSchema": {
                "$schema": "https://json-schema.org/draft/2020-12/schema",
                "type": "object",
                "properties": {
                    "transport": {"type": "string", "enum": ["http", "stdio_mcp", "cli"]},
                    "memory": {"type": ["object", "null"]},
                },
                "required": ["transport", "memory"],
            },
            "annotations": {
                "readOnlyHint": false,
                "destructiveHint": false,
                "idempotentHint": false,
                "openWorldHint": false,
            },
        }),
        json!({
            "name": "hzr_search",
            "title": "Search HZR Workspace",
            "description": "Find code in this repository through the one canonical HZR index. \
        Use semantic for intent or concepts, exact for literal symbols and error text, and auto \
        when either strategy is acceptable. Results may report an exact fallback while the \
        semantic index warms.",
            "inputSchema": {
                "$schema": "https://json-schema.org/draft/2020-12/schema",
                "type": "object",
                "additionalProperties": false,
                "properties": {
                    "query": {
                        "type": "string",
                        "minLength": 1,
                        "description": "Intent or exact pattern to find.",
                    },
                    "path": {
                        "type": "string",
                        "minLength": 1,
                        "description": "Optional path inside the fixed workspace used to narrow the search; it cannot widen scope.",
                    },
                    "mode": {
                        "type": "string",
                        "enum": ["auto", "semantic", "exact"],
                        "default": "auto",
                        "description": "semantic searches by intent; exact preserves a literal pattern; auto selects or safely degrades.",
                    },
                    "limit": {
                        "type": "integer",
                        "minimum": 1,
                        "maximum": 50,
                        "default": 10,
                        "description": "Maximum search hits returned.",
                    },
                    "include_content": {
                        "type": "boolean",
                        "default": false,
                        "description": "Include bounded matching snippets when true. Keep false for file discovery.",
                    },
                },
                "required": ["query"],
            },
            "outputSchema": {
                "$schema": "https://json-schema.org/draft/2020-12/schema",
                "type": "object",
                "properties": {
                    "query": {"type": "string"},
                    "path": {"type": "string"},
                    "total_hits": {"type": "integer", "minimum": 0},
                    "shown_hits": {"type": "integer", "minimum": 0},
                    "hits": {"type": "array", "items": {"type": "object"}},
                    "strategy": {"type": "string", "enum": ["fork_rgai_adaptive", "fork_rgai_builtin"]},
                    "fallback_reason": {"type": "string"},
                },
                "required": ["query", "path", "total_hits", "shown_hits", "hits", "strategy"],
            },
            "annotations": {
                "readOnlyHint": true,
                "destructiveHint": false,
                "openWorldHint": false,
            },
        }),
        json!({
            "name": "hzr_context_plan",
            "title": "Plan HZR Context",
            "description": "Build one bounded graph-first evidence plan for unfamiliar, \
        architectural or cross-cutting work. It fuses structural code candidates, the canonical \
        HZR search index and durable memory, returning selected files, contents, token budget, \
        provenance and explicit degradation warnings.",
            "inputSchema": {
                "$schema": "https://json-schema.org/draft/2020-12/schema",
                "type": "object",
                "additionalProperties": false,
                "properties": {
                    "intent": {
                        "type": "string",
                        "minLength": 1,
                        "description": "The exact task or architectural question to gather evidence for.",
                    },
                    "path": {
                        "type": "string",
                        "minLength": 1,
                        "description": "Optional path inside the fixed workspace used to narrow planning.",
                    },
                    "topic": {
                        "type": "string",
                        "minLength": 1,
                        "maxLength": 64,
                        "pattern": "^[a-z0-9]+(?:-[a-z0-9]+)*$",
                        "description": "Optional project-memory kind used to narrow durable context.",
                    },
                    "search_limit": {
                        "type": "integer",
                        "minimum": 1,
                        "maximum": 50,
                        "default": 10,
                        "description": "Maximum code candidates before fusion.",
                    },
                    "memory_limit": {
                        "type": "integer",
                        "minimum": 1,
                        "maximum": 50,
                        "default": 5,
                        "description": "Maximum project-memory candidates before fusion.",
                    },
                },
                "required": ["intent"],
            },
            "outputSchema": {
                "$schema": "https://json-schema.org/draft/2020-12/schema",
                "type": "object",
                "properties": {
                    "pack": {"type": "object"},
                    "contents": {"type": "object", "additionalProperties": {"type": "string"}},
                    "warnings": {"type": "array", "items": {"type": "object"}},
                    "planner": {"type": "object"},
                },
                "required": ["pack", "contents", "warnings"],
            },
            "annotations": {
                "readOnlyHint": true,
                "destructiveHint": false,
                "openWorldHint": false,
            },
        }),
        json!({
            "name": "hzr_codec",
            "title": "Compile an HZR Response-Density Contract",
            "description": "Remove exact duplicate paragraphs from a long answer while provably \
        preserving code, commands, paths, identifiers, errors, numbers and URLs. It is a \
        structural transform, not a summariser: it never rewords prose, so text with no \
        repeated paragraph comes back byte-identical and that is a correct result, not a \
        failure. Use profile \"shadow\" to measure the counterfactual without changing the \
        text. Protected spans are verified after the transform: if any of them changed, the \
        call fails rather than returning altered technical content.",
            "inputSchema": {
                "$schema": "https://json-schema.org/draft/2020-12/schema",
                "type": "object",
                "additionalProperties": false,
                "properties": {
                    "content": {
                        "type": "string",
                        "minLength": 1,
                        "description": "The prose to compile. Code blocks, inline code, paths, flags, URLs, hashes and versions inside it are protected and returned unchanged.",
                    },
                    "fidelity": {
                        "type": "string",
                        "enum": ["exact", "lossless_structural", "semantic", "summary"],
                        "default": "semantic",
                        "description": "How much rewriting is permitted. exact returns the input untouched; summary permits the most aggressive rewrite.",
                    },
                    "risk": {
                        "type": "string",
                        "enum": ["low", "medium", "high", "irreversible"],
                        "default": "low",
                        "description": "Risk of the action being described. high and irreversible force full detail regardless of profile.",
                    },
                    "profile": {
                        "type": "string",
                        "enum": ["off", "safe", "adaptive", "compact", "shadow"],
                        "default": "adaptive",
                        "description": "shadow computes the counterfactual without changing the content, which is the only honest way to measure the codec's value.",
                    },
                },
                "required": ["content"],
            },
            "outputSchema": {
                "$schema": "https://json-schema.org/draft/2020-12/schema",
                "type": "object",
                "properties": {
                    "content": {"type": "string"},
                    "changed": {"type": "boolean"},
                    "profile": {"type": "string"},
                    "protected_spans": {"type": "array", "items": {"type": "object"}},
                    "counterfactual": {
                        "type": "object",
                        "description": "Present in shadow profile: the sizes the transform would have produced.",
                        "properties": {
                            "input_bytes": {"type": "integer", "minimum": 0},
                            "output_bytes": {"type": "integer", "minimum": 0},
                            "saved_bytes": {"type": "integer", "minimum": 0},
                            "would_change": {"type": "boolean"},
                        },
                    },
                },
                "required": ["content", "changed", "profile", "protected_spans"],
            },
            "annotations": {
                "readOnlyHint": true,
                "destructiveHint": false,
                "openWorldHint": false,
            },
        }),
    ]
}
