use serde_json::{Value, json};

fn strict_object(properties: Value, required: &[&str]) -> Value {
    json!({
        "type": "object",
        "additionalProperties": false,
        "properties": properties,
        "required": required,
    })
}

fn nullable_string() -> Value {
    json!({"type": ["string", "null"]})
}

fn memory_source_schema() -> Value {
    json!({
        "anyOf": [
            strict_object(
                json!({
                    "type": {"const": "claude_code"},
                    "session_id": {"type": "string"},
                    "file_path": nullable_string()
                }),
                &["type", "session_id", "file_path"]
            ),
            strict_object(
                json!({
                    "type": {"const": "conversation"},
                    "thread_id": {"type": "string"}
                }),
                &["type", "thread_id"]
            ),
            strict_object(json!({"type": {"const": "manual"}}), &["type"])
        ]
    })
}

fn memory_record_schema() -> Value {
    strict_object(
        json!({
            "score": {"type": ["number", "null"]},
            "id": {"type": "string"},
            "created_at": {"type": "string"},
            "updated_at": {"type": "string"},
            "last_accessed": {"type": "string"},
            "access_count": {"type": "integer", "minimum": 0},
            "weight": {"type": "number"},
            "topic": {"type": "string"},
            "summary": {"type": "string"},
            "raw_excerpt": nullable_string(),
            "keywords": {"type": "array", "items": {"type": "string"}},
            "importance": {"type": "string", "enum": ["critical", "high", "medium", "low"]},
            "source": memory_source_schema(),
            "related_ids": {"type": "array", "items": {"type": "string"}},
            "scope": {"type": "string", "enum": ["user", "project", "org"]}
        }),
        &[
            "score",
            "id",
            "created_at",
            "updated_at",
            "last_accessed",
            "access_count",
            "weight",
            "topic",
            "summary",
            "raw_excerpt",
            "keywords",
            "importance",
            "source",
            "related_ids",
            "scope",
        ],
    )
}

fn search_hit_schema() -> Value {
    strict_object(
        json!({
            "path": {"type": "string"},
            "score": {"type": "number"},
            "matched_lines": {"type": "integer", "minimum": 0},
            "snippets": {
                "type": "array",
                "items": strict_object(
                    json!({
                        "lines": {
                            "type": "array",
                            "items": strict_object(
                                json!({
                                    "line": {"type": "integer", "minimum": 0},
                                    "text": {"type": "string"}
                                }),
                                &["line", "text"]
                            )
                        },
                        "matched_terms": {"type": "array", "items": {"type": "string"}}
                    }),
                    &["lines"]
                )
            }
        }),
        &["path", "score", "matched_lines", "snippets"],
    )
}

fn token_count_schema() -> Value {
    strict_object(
        json!({
            "value": {"type": "integer", "minimum": 0},
            "source": {"type": "string", "enum": ["provider", "model_tokenizer", "estimate"]}
        }),
        &["value", "source"],
    )
}

fn context_candidate_schema() -> Value {
    strict_object(
        json!({
            "id": {"type": "string"},
            "source": {"type": "string", "enum": ["exact", "index", "context", "memory"]},
            "content_ref": {"type": "string"},
            "path": nullable_string(),
            "symbol": nullable_string(),
            "symbol_unavailable_reason": {
                "type": "string",
                "enum": ["whole_file_candidate", "outline_unavailable", "no_enclosing_symbol", "not_applicable"]
            },
            "line_start": {"type": ["integer", "null"], "minimum": 0},
            "line_end": {"type": ["integer", "null"], "minimum": 0},
            "source_rank": {"type": "integer", "minimum": 0},
            "relevance": {"type": "number"},
            "tokens": token_count_schema(),
            "freshness": {"type": "string"},
            "trust": {"type": "string"},
            "provenance": strict_object(
                json!({
                    "source": {"type": "string"},
                    "content_hash": {"type": "string"},
                    "generation": nullable_string(),
                    "canonical_ref": nullable_string(),
                    "derived_by": nullable_string()
                }),
                &["source", "content_hash", "generation", "canonical_ref", "derived_by"]
            )
        }),
        &[
            "id",
            "source",
            "content_ref",
            "path",
            "symbol",
            "line_start",
            "line_end",
            "source_rank",
            "relevance",
            "tokens",
            "freshness",
            "trust",
            "provenance",
        ],
    )
}

fn context_pack_schema() -> Value {
    strict_object(
        json!({
            "selected": {"type": "array", "items": context_candidate_schema()},
            "rejected": {
                "type": "array",
                "items": strict_object(
                    json!({"candidate_id": {"type": "string"}, "reason": {"type": "string"}}),
                    &["candidate_id", "reason"]
                )
            },
            "used": token_count_schema(),
            "hard_limit": {"type": "integer", "minimum": 0},
            "coverage": {"type": "number"},
            "confidence": {"type": "number"},
            "budget_exceeded": {"type": "boolean"}
        }),
        &[
            "selected",
            "rejected",
            "used",
            "hard_limit",
            "coverage",
            "confidence",
            "budget_exceeded",
        ],
    )
}

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
                "additionalProperties": false,
                "properties": {
                    "count": {"type": "integer", "minimum": 0},
                    "total_matches": {"type": "integer", "minimum": 0},
                    "memories": {"type": "array", "items": memory_record_schema()},
                },
                "required": ["count", "total_matches", "memories"],
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
                "additionalProperties": false,
                "properties": {
                    "transport": {"type": "string", "enum": ["http", "stdio_mcp", "cli"]},
                    "memory": {"anyOf": [memory_record_schema(), {"type": "null"}]},
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
            "name": "hzr_memory_forget",
            "title": "Forget HZR Memory",
            "description": "Delete one memory only after HZR verifies that it belongs to the selected project or global namespace.",
            "inputSchema": {
                "$schema": "https://json-schema.org/draft/2020-12/schema",
                "type": "object",
                "additionalProperties": false,
                "properties": {
                    "id": {"type": "string", "minLength": 1, "maxLength": 128},
                    "scope": {
                        "type": "string",
                        "enum": ["project", "global"],
                        "default": "project"
                    }
                },
                "required": ["id"]
            },
            "outputSchema": {
                "$schema": "https://json-schema.org/draft/2020-12/schema",
                "type": "object",
                "additionalProperties": false,
                "properties": {
                    "affected_ids": {"type": "array", "items": {"type": "string"}},
                    "dry_run": {"type": "boolean"}
                },
                "required": ["affected_ids", "dry_run"]
            },
            "annotations": {
                "readOnlyHint": false,
                "destructiveHint": true,
                "idempotentHint": false,
                "openWorldHint": false
            }
        }),
        json!({
            "name": "hzr_memory_update",
            "title": "Update HZR Memory",
            "description": "Replace one memory in place only after HZR verifies namespace ownership.",
            "inputSchema": {
                "$schema": "https://json-schema.org/draft/2020-12/schema",
                "type": "object",
                "additionalProperties": false,
                "properties": {
                    "id": {"type": "string", "minLength": 1, "maxLength": 128},
                    "content": {"type": "string", "minLength": 1},
                    "importance": {"type": "string", "enum": ["critical", "high", "medium", "low"]},
                    "keywords": {"type": "array", "maxItems": 32, "items": {"type": "string", "minLength": 1}},
                    "scope": {
                        "type": "string",
                        "enum": ["project", "global"],
                        "default": "project"
                    }
                },
                "required": ["id", "content"]
            },
            "outputSchema": {
                "$schema": "https://json-schema.org/draft/2020-12/schema",
                "type": "object",
                "additionalProperties": false,
                "properties": {
                    "affected_ids": {"type": "array", "items": {"type": "string"}},
                    "dry_run": {"type": "boolean"}
                },
                "required": ["affected_ids", "dry_run"]
            },
            "annotations": {
                "readOnlyHint": false,
                "destructiveHint": true,
                "idempotentHint": true,
                "openWorldHint": false
            }
        }),
        json!({
            "name": "hzr_memory_prune",
            "title": "Prune HZR Memory",
            "description": "Preview or delete low-weight memories only inside the selected HZR namespace. Dry-run defaults to true.",
            "inputSchema": {
                "$schema": "https://json-schema.org/draft/2020-12/schema",
                "type": "object",
                "additionalProperties": false,
                "properties": {
                    "threshold": {"type": "number", "minimum": 0, "maximum": 1, "default": 0.1},
                    "dry_run": {"type": "boolean", "default": true},
                    "scope": {
                        "type": "string",
                        "enum": ["project", "global"],
                        "default": "project"
                    }
                },
                "required": []
            },
            "outputSchema": {
                "$schema": "https://json-schema.org/draft/2020-12/schema",
                "type": "object",
                "additionalProperties": false,
                "properties": {
                    "affected_ids": {"type": "array", "items": {"type": "string"}},
                    "dry_run": {"type": "boolean"}
                },
                "required": ["affected_ids", "dry_run"]
            },
            "annotations": {
                "readOnlyHint": false,
                "destructiveHint": true,
                "idempotentHint": false,
                "openWorldHint": false
            }
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
                "additionalProperties": false,
                "properties": {
                    "query": {"type": "string"},
                    "path": {"type": "string"},
                    "total_hits": {"type": "integer", "minimum": 0},
                    "shown_hits": {"type": "integer", "minimum": 0},
                    "scanned_files": {"type": "integer", "minimum": 0},
                    "skipped_large": {"type": "integer", "minimum": 0},
                    "skipped_binary": {"type": "integer", "minimum": 0},
                    "hits": {"type": "array", "items": search_hit_schema()},
                    "effective_mode": {"type": "string", "enum": ["auto", "semantic", "exact"]},
                    "strategy": {"type": "string", "enum": ["fork_rgai_adaptive", "fork_rgai_builtin"]},
                    "fallback_reason": {"type": "string"},
                    "next_step": {"type": "string"},
                },
                "required": ["query", "path", "total_hits", "shown_hits", "scanned_files", "skipped_large", "skipped_binary", "hits", "effective_mode", "strategy"],
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
                "additionalProperties": false,
                "properties": {
                    "pack": context_pack_schema(),
                    "contents": {"type": "object", "additionalProperties": {"type": "string"}},
                    "warnings": {
                        "type": "array",
                        "items": strict_object(
                            json!({
                                "code": {"type": "string", "enum": ["planner_fallback", "planner_unavailable", "search_degraded", "search_unavailable", "memory_unavailable", "content_unavailable", "outline_unavailable", "warnings_truncated"]},
                                "message": {"type": "string"}
                            }),
                            &["code", "message"]
                        )
                    },
                    "planner": strict_object(
                        json!({
                            "pipeline_version": nullable_string(),
                            "semantic_backend_used": nullable_string(),
                            "graph_candidate_count": {"type": ["integer", "null"], "minimum": 0},
                            "semantic_hit_count": {"type": ["integer", "null"], "minimum": 0},
                            "candidates_total": {"type": "integer", "minimum": 0},
                            "candidates_selected": {"type": "integer", "minimum": 0},
                            "estimated_tokens_used": {"type": "integer", "minimum": 0},
                            "token_budget": {"type": "integer", "minimum": 0}
                        }),
                        &["pipeline_version", "semantic_backend_used", "graph_candidate_count", "semantic_hit_count", "candidates_total", "candidates_selected", "estimated_tokens_used", "token_budget"]
                    ),
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
                "additionalProperties": false,
                "properties": {
                    "content": {"type": "string"},
                    "changed": {"type": "boolean"},
                    "profile": {"type": "string"},
                    "protected_spans": {
                        "type": "array",
                        "items": strict_object(
                            json!({
                                "start": {"type": "integer", "minimum": 0},
                                "end": {"type": "integer", "minimum": 0},
                                "kind": {"type": "string"}
                            }),
                            &["start", "end", "kind"]
                        )
                    },
                    "counterfactual": {
                        "type": "object",
                        "additionalProperties": false,
                        "description": "Present in shadow profile: the sizes the transform would have produced.",
                        "properties": {
                            "input_bytes": {"type": "integer", "minimum": 0},
                            "output_bytes": {"type": "integer", "minimum": 0},
                            "saved_bytes": {"type": "integer", "minimum": 0},
                            "would_change": {"type": "boolean"},
                        },
                        "required": ["input_bytes", "output_bytes", "saved_bytes", "would_change"],
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
