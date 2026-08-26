use serde_json::{Value, json};

pub(super) const MCP_EXEC_TIMEOUT_MAX_MS: u64 = 29_500;
pub(super) const MCP_PATH_MAX_BYTES: usize = 4096;
pub(super) const MCP_PATCH_BLOCK_MAX_BYTES: usize = 65_536;
pub(super) const MCP_CREATE_CONTENT_MAX_BYTES: usize = 192 * 1024;

#[derive(Clone, Copy, Debug)]
pub(super) struct ToolContract {
    pub name: &'static str,
    pub kind: ToolKind,
}

#[derive(Clone, Copy, Debug)]
pub(super) enum ToolKind {
    MemoryRecall,
    MemoryStore,
    MemoryForget,
    MemoryUpdate,
    MemoryPrune,
    Search,
    ContextPlan,
    Codec,
    Read,
    Write,
    Exec,
    Observability,
    Doctor,
}

pub(super) const TOOL_CONTRACTS: &[ToolContract] = &[
    ToolContract {
        name: "hzr_memory_recall",
        kind: ToolKind::MemoryRecall,
    },
    ToolContract {
        name: "hzr_memory_store",
        kind: ToolKind::MemoryStore,
    },
    ToolContract {
        name: "hzr_memory_forget",
        kind: ToolKind::MemoryForget,
    },
    ToolContract {
        name: "hzr_memory_update",
        kind: ToolKind::MemoryUpdate,
    },
    ToolContract {
        name: "hzr_memory_prune",
        kind: ToolKind::MemoryPrune,
    },
    ToolContract {
        name: "hzr_search",
        kind: ToolKind::Search,
    },
    ToolContract {
        name: "hzr_context_plan",
        kind: ToolKind::ContextPlan,
    },
    ToolContract {
        name: "hzr_codec",
        kind: ToolKind::Codec,
    },
    ToolContract {
        name: "hzr_read",
        kind: ToolKind::Read,
    },
    ToolContract {
        name: "hzr_write",
        kind: ToolKind::Write,
    },
    ToolContract {
        name: "hzr_exec",
        kind: ToolKind::Exec,
    },
    ToolContract {
        name: "hzr_observability",
        kind: ToolKind::Observability,
    },
    ToolContract {
        name: "hzr_doctor",
        kind: ToolKind::Doctor,
    },
];

pub(super) fn tool_contract(name: &str) -> Option<&'static ToolContract> {
    TOOL_CONTRACTS.iter().find(|contract| contract.name == name)
}

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

fn fork_result_properties(content_key: &str, content_schema: Value) -> Value {
    json!({
        (content_key): content_schema,
        "stderr": {"type": "string"},
        "termination": {"type": "string", "enum": ["exited", "signaled", "timed_out", "cancelled"]},
        "exit_code": {"type": ["integer", "null"]},
        "signal": {"type": ["integer", "null"]},
        "duration_ms": {"type": "integer", "minimum": 0},
        "stdout_sha256": {"type": "string"},
        "stderr_sha256": {"type": "string"},
        "stdout_truncated": {"type": "boolean"},
        "stderr_truncated": {"type": "boolean"}
    })
}

fn fork_result_schema(content_key: &str, content_schema: Value) -> Value {
    strict_object(
        fork_result_properties(content_key, content_schema),
        &[
            content_key,
            "stderr",
            "termination",
            "exit_code",
            "signal",
            "duration_ms",
            "stdout_sha256",
            "stderr_sha256",
            "stdout_truncated",
            "stderr_truncated",
        ],
    )
}

fn engine_health_schema() -> Value {
    strict_object(
        json!({
            "name": {"type": "string"},
            "version": nullable_string(),
            "state": {"type": "string", "enum": ["ready", "degraded", "rebuilding", "stopped"]},
            "detail": nullable_string()
        }),
        &["name", "version", "state", "detail"],
    )
}

fn health_schema() -> Value {
    strict_object(
        json!({
            "protocol_version": {"type": "integer", "minimum": 1},
            "hzr_version": {"type": "string"},
            "state": {"type": "string", "enum": ["ready", "degraded", "rebuilding", "stopped"]},
            "workspace_root": nullable_string(),
            "engines": {"type": "array", "items": engine_health_schema()},
            "capabilities": {"type": "array", "items": {"type": "string"}}
        }),
        &[
            "protocol_version",
            "hzr_version",
            "state",
            "workspace_root",
            "engines",
            "capabilities",
        ],
    )
}

fn doctor_schema() -> Value {
    strict_object(
        json!({
            "hzr_version": {"type": "string"},
            "config_path": {"type": "string"},
            "data_dir": {"type": "string"},
            "workspace": {"type": "string"},
            "healthy": {"type": "boolean"},
            "checks": {
                "type": "array",
                "items": strict_object(
                    json!({
                        "name": {"type": "string"},
                        "status": {"type": "string", "enum": ["pass", "warning", "error"]},
                        "detail": {"type": "string"}
                    }),
                    &["name", "status", "detail"]
                )
            },
            "client_workspace_bindings": {
                "type": "array",
                "items": {"type": "object"}
            },
            "response_codec_coverage": {
                "type": "array",
                "items": {"type": "object"}
            },
            "repair": {"type": ["object", "null"]}
        }),
        &[
            "hzr_version",
            "config_path",
            "data_dir",
            "workspace",
            "healthy",
            "checks",
            "client_workspace_bindings",
            "response_codec_coverage",
        ],
    )
}

fn raw_tool_definitions() -> Vec<Value> {
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
                    "strategy": {
                        "type": "string",
                        "enum": [
                            "fork_rgai_adaptive",
                            "fork_rgai_builtin",
                            "fork_rgai_grepai",
                            "fork_rgai_ripgrep",
                            "fork_rgai_files"
                        ]
                    },
                    "fallback_code": {
                        "type": "string",
                        "enum": ["legacy_index_requires_migration", "semantic_index_unavailable", "grepai_unavailable", "ripgrep_unavailable"]
                    },
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
        call fails rather than returning altered technical content. Claude and Codex do not \
        expose a global response-replacement hook. The returned tool payload can earn \
        estimated codec-token credit when it is smaller, but it never proves a later final \
        response was replaced and never earns provider-billed credit by itself.",
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
                    "coverage_state": {
                        "type": "string",
                        "enum": ["applied", "shadow_measured", "instructed", "unavailable"]
                    },
                    "global_response_replacement_confirmed": {"type": "boolean"},
                    "estimated_token_credit_eligible": {"type": "boolean"},
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
                "required": [
                    "content", "changed", "profile", "protected_spans", "coverage_state",
                    "global_response_replacement_confirmed", "estimated_token_credit_eligible"
                ],
            },
            "annotations": {
                "readOnlyHint": true,
                "destructiveHint": false,
                "openWorldHint": false,
            },
        }),
        json!({
            "name": "hzr_read",
            "title": "Read through HZR",
            "description": "Read bounded exact content through the daemon-owned fork-core path. The response preserves termination, hashes and truncation so omitted bytes cannot look complete.",
            "inputSchema": {
                "$schema": "https://json-schema.org/draft/2020-12/schema",
                "type": "object",
                "additionalProperties": false,
                "properties": {
                    "path": {"type": "string", "minLength": 1, "maxLength": MCP_PATH_MAX_BYTES},
                    "outline": {"type": "boolean", "default": false},
                    "from": {"type": "integer", "minimum": 1},
                    "to": {"type": "integer", "minimum": 1},
                    "max_lines": {"type": "integer", "minimum": 1}
                },
                "required": ["path"]
            },
            "outputSchema": fork_result_schema("content", json!({"type": "string"})),
            "annotations": {"readOnlyHint": true, "destructiveHint": false, "openWorldHint": false}
        }),
        json!({
            "name": "hzr_write",
            "title": "Write through HZR",
            "description": "Apply an atomic patch or non-overwriting create through daemon-owned fork-core. Patch always uses CAS with two bounded retries and returns the typed write receipt.",
            "inputSchema": {
                "$schema": "https://json-schema.org/draft/2020-12/schema",
                "type": "object",
                "additionalProperties": false,
                "properties": {
                    "operation": {"type": "string", "enum": ["patch", "create"]},
                    "path": {"type": "string", "minLength": 1, "maxLength": MCP_PATH_MAX_BYTES},
                    "old": {"type": "string", "maxLength": MCP_PATCH_BLOCK_MAX_BYTES},
                    "new": {"type": "string", "maxLength": MCP_PATCH_BLOCK_MAX_BYTES},
                    "content": {"type": "string", "maxLength": MCP_CREATE_CONTENT_MAX_BYTES},
                    "cas": {"type": "boolean", "const": true, "default": true}
                },
                "required": ["operation", "path"],
                "allOf": [
                    {
                        "if": {"properties": {"operation": {"const": "patch"}}},
                        "then": {"required": ["old", "new"], "not": {"required": ["content"]}}
                    },
                    {
                        "if": {"properties": {"operation": {"const": "create"}}},
                        "then": {"required": ["content"], "not": {"anyOf": [{"required": ["old"]}, {"required": ["new"]}]}}
                    }
                ]
            },
            "outputSchema": fork_result_schema("receipt", json!({"type": "object"})),
            "annotations": {"readOnlyHint": false, "destructiveHint": true, "idempotentHint": true, "openWorldHint": false}
        }),
        json!({
            "name": "hzr_exec",
            "title": "Execute through HZR Policy",
            "description": "Run one shell command through the daemon-owned policy, rewrite and accounting pipeline. Approval-required and denied decisions are returned as typed outcomes; MCP never falls back to a direct shell.",
            "inputSchema": {
                "$schema": "https://json-schema.org/draft/2020-12/schema",
                "type": "object",
                "additionalProperties": false,
                "properties": {
                    "command": {"type": "string", "minLength": 1},
                    "timeout_ms": {"type": "integer", "minimum": 1, "maximum": MCP_EXEC_TIMEOUT_MAX_MS}
                },
                "required": ["command"]
            },
            "outputSchema": {
                "$schema": "https://json-schema.org/draft/2020-12/schema",
                "type": "object",
                "additionalProperties": false,
                "properties": {
                    "outcome": {"type": "string", "enum": ["completed", "executed_accounting_incomplete", "not_started"]},
                    "result": {"type": "object"},
                    "disposition": {"type": "object"}
                },
                "required": ["outcome"]
            },
            "annotations": {"readOnlyHint": false, "destructiveHint": true, "idempotentHint": false, "openWorldHint": true}
        }),
        json!({
            "name": "hzr_observability",
            "title": "Observe HZR Health",
            "description": "Return the daemon's typed health, engine state, capabilities and exact bound workspace without scraping CLI or UI text.",
            "inputSchema": {
                "$schema": "https://json-schema.org/draft/2020-12/schema",
                "type": "object", "additionalProperties": false, "properties": {}, "required": []
            },
            "outputSchema": health_schema(),
            "annotations": {"readOnlyHint": true, "destructiveHint": false, "openWorldHint": false}
        }),
        json!({
            "name": "hzr_doctor",
            "title": "Diagnose HZR Desired State",
            "description": "Run the typed desired-state doctor for the exact MCP workspace, including ownership, binding, ledger and engine checks.",
            "inputSchema": {
                "$schema": "https://json-schema.org/draft/2020-12/schema",
                "type": "object", "additionalProperties": false, "properties": {}, "required": []
            },
            "outputSchema": doctor_schema(),
            "annotations": {"readOnlyHint": true, "destructiveHint": false, "openWorldHint": false}
        }),
    ]
}

pub(super) fn tool_definitions() -> Vec<Value> {
    let definitions = raw_tool_definitions();
    for definition in &definitions {
        let name = definition["name"]
            .as_str()
            .expect("internal MCP definition has a name");
        debug_assert!(
            tool_contract(name).is_some(),
            "internal MCP definition has no handler contract: {name}"
        );
    }
    debug_assert_eq!(definitions.len(), TOOL_CONTRACTS.len());
    definitions
}

pub(super) fn validate_tool_input(name: &str, value: &Value) -> Result<(), String> {
    validate_tool_payload(name, "inputSchema", value)
}

pub(super) fn validate_tool_output(name: &str, value: &Value) -> Result<(), String> {
    validate_tool_payload(name, "outputSchema", value)
}

fn validate_tool_payload(name: &str, schema_key: &str, value: &Value) -> Result<(), String> {
    let definitions = raw_tool_definitions();
    let definition = definitions
        .iter()
        .find(|definition| definition["name"] == name)
        .ok_or_else(|| format!("unknown tool: {name}"))?;
    validate_schema(&definition[schema_key], value, name)
}

fn validate_schema(schema: &Value, value: &Value, path: &str) -> Result<(), String> {
    if let Some(options) = schema.get("anyOf").and_then(Value::as_array) {
        if !options
            .iter()
            .any(|option| validate_schema(option, value, path).is_ok())
        {
            return Err(format!("{path} matched no allowed schema branch"));
        }
    }
    if let Some(expected) = schema.get("const") {
        if expected != value {
            return Err(format!("{path} must equal {expected}"));
        }
    }
    if let Some(values) = schema.get("enum").and_then(Value::as_array) {
        if !values.contains(value) {
            return Err(format!("{path} is outside the advertised enum"));
        }
    }
    if let Some(kind) = schema.get("type") {
        let kinds = kind
            .as_array()
            .cloned()
            .unwrap_or_else(|| vec![kind.clone()]);
        let matches = kinds.iter().any(|kind| match kind.as_str() {
            Some("object") => value.is_object(),
            Some("array") => value.is_array(),
            Some("string") => value.is_string(),
            Some("integer") => value.as_i64().is_some() || value.as_u64().is_some(),
            Some("number") => value.is_number(),
            Some("boolean") => value.is_boolean(),
            Some("null") => value.is_null(),
            _ => false,
        });
        if !matches {
            return Err(format!("{path} has the wrong JSON type"));
        }
    }
    if let Some(number) = value.as_f64() {
        if let Some(minimum) = schema.get("minimum").and_then(Value::as_f64) {
            if number < minimum {
                return Err(format!("{path} is below the advertised minimum"));
            }
        }
        if let Some(maximum) = schema.get("maximum").and_then(Value::as_f64) {
            if number > maximum {
                return Err(format!("{path} exceeds the advertised maximum"));
            }
        }
    }
    if let Some(text) = value.as_str() {
        let character_count = text.chars().count() as u64;
        if let Some(minimum) = schema.get("minLength").and_then(Value::as_u64) {
            if character_count < minimum {
                return Err(format!("{path} is shorter than the advertised minimum"));
            }
        }
        if let Some(maximum) = schema.get("maxLength").and_then(Value::as_u64) {
            if character_count > maximum {
                return Err(format!("{path} exceeds the advertised maximum length"));
            }
        }
        if schema.get("pattern").and_then(Value::as_str) == Some("^[a-z0-9]+(?:-[a-z0-9]+)*$")
            && !is_kebab_case(text)
        {
            return Err(format!("{path} does not match the advertised pattern"));
        }
    }
    if let Some(items) = value.as_array() {
        if let Some(maximum) = schema.get("maxItems").and_then(Value::as_u64) {
            if items.len() as u64 > maximum {
                return Err(format!("{path} contains too many items"));
            }
        }
        if let Some(item_schema) = schema.get("items") {
            for (index, item) in items.iter().enumerate() {
                validate_schema(item_schema, item, &format!("{path}[{index}]"))?;
            }
        }
    }
    if let Some(object) = value.as_object() {
        if let Some(required) = schema.get("required").and_then(Value::as_array) {
            for key in required.iter().filter_map(Value::as_str) {
                if !object.contains_key(key) {
                    return Err(format!("{path} is missing required property `{key}`"));
                }
            }
        }
        let properties = schema.get("properties").and_then(Value::as_object);
        for (key, child) in object {
            if let Some(child_schema) = properties.and_then(|properties| properties.get(key)) {
                validate_schema(child_schema, child, &format!("{path}.{key}"))?;
            } else if schema.get("additionalProperties") == Some(&Value::Bool(false)) {
                return Err(format!("{path} contains unknown property `{key}`"));
            } else if let Some(additional) = schema
                .get("additionalProperties")
                .filter(|additional| additional.is_object())
            {
                validate_schema(additional, child, &format!("{path}.{key}"))?;
            }
        }
    }
    if let Some(negated) = schema.get("not") {
        if validate_schema(negated, value, path).is_ok() {
            return Err(format!("{path} matches a forbidden schema branch"));
        }
    }
    if let Some(parts) = schema.get("allOf").and_then(Value::as_array) {
        for part in parts {
            if let Some(condition) = part.get("if") {
                if validate_schema(condition, value, path).is_ok() {
                    if let Some(consequence) = part.get("then") {
                        validate_schema(consequence, value, path)?;
                    }
                }
            } else {
                validate_schema(part, value, path)?;
            }
        }
    }
    Ok(())
}

fn is_kebab_case(value: &str) -> bool {
    !value.is_empty()
        && value.split('-').all(|part| {
            !part.is_empty()
                && part
                    .bytes()
                    .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit())
        })
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use serde::Deserialize;

    #[derive(Deserialize)]
    struct CapabilityContract {
        mcp_tools: Vec<CapabilityTool>,
    }

    #[derive(Deserialize)]
    struct CapabilityTool {
        name: String,
    }

    #[test]
    fn acceptance_gate_mcp_inventory_matches_agent_capability_ssot() {
        let contract: CapabilityContract = serde_json::from_str(include_str!(
            "../../../../contracts/agent-capabilities.json"
        ))
        .expect("capability contract");
        let mut expected = contract
            .mcp_tools
            .into_iter()
            .map(|tool| tool.name)
            .collect::<Vec<_>>();
        let mut actual = super::tool_definitions()
            .into_iter()
            .map(|tool| {
                tool.get("name")
                    .and_then(serde_json::Value::as_str)
                    .expect("MCP tool name")
                    .to_owned()
            })
            .collect::<Vec<_>>();
        expected.sort();
        actual.sort();

        assert_eq!(actual, expected);
    }

    #[test]
    fn acceptance_gate_advertised_input_schema_is_the_runtime_parser_contract() {
        for definition in super::tool_definitions() {
            let name = definition["name"].as_str().expect("tool name");
            let schema = &definition["inputSchema"];
            super::tool_contract(name).expect("tool handler contract");
            let mut sample = sample_input(name);
            let advertised = super::validate_tool_input(name, &sample);
            assert!(
                advertised.is_ok(),
                "advertised sample for {name} failed: {advertised:?}"
            );

            sample
                .as_object_mut()
                .expect("sample object")
                .insert("schema_drift".to_owned(), serde_json::json!(true));
            assert!(
                super::validate_tool_input(name, &sample).is_err(),
                "{name} accepted an unadvertised field"
            );

            let properties = schema["properties"].as_object().expect("properties");
            for key in properties.keys() {
                let mut wrong_type = sample_input(name);
                wrong_type
                    .as_object_mut()
                    .expect("sample object")
                    .insert(key.clone(), serde_json::Value::Null);
                assert!(
                    super::validate_tool_input(name, &wrong_type).is_err(),
                    "{name}.{key} accepted a type excluded by tools/list"
                );
            }
            for required in schema["required"]
                .as_array()
                .expect("required")
                .iter()
                .map(|value| value.as_str().expect("required field"))
            {
                let mut missing = sample_input(name);
                missing
                    .as_object_mut()
                    .expect("sample object")
                    .remove(required);
                assert!(
                    super::validate_tool_input(name, &missing).is_err(),
                    "{name} accepted missing required field {required}"
                );
            }
            assert_eq!(schema["additionalProperties"], false);
        }
    }

    #[expect(
        clippy::panic,
        reason = "fixture must fail loudly when an advertised tool has no sample"
    )]
    fn sample_input(name: &str) -> serde_json::Value {
        match name {
            "hzr_memory_recall" => serde_json::json!({
                "query":"decision", "topic":"architecture", "keyword":"ledger",
                "limit":1, "scope":"project"
            }),
            "hzr_memory_store" => serde_json::json!({
                "topic":"architecture", "content":"durable", "importance":"high",
                "keywords":["ledger"], "scope":"project"
            }),
            "hzr_memory_forget" => serde_json::json!({"id":"memory-1", "scope":"project"}),
            "hzr_memory_update" => serde_json::json!({
                "id":"memory-1", "content":"replacement", "importance":"medium",
                "keywords":["ledger"], "scope":"project"
            }),
            "hzr_memory_prune" => {
                serde_json::json!({"threshold":0.2, "dry_run":true, "scope":"project"})
            }
            "hzr_search" => serde_json::json!({
                "query":"Ledger", "path":"crates", "mode":"exact", "limit":1,
                "include_content":true
            }),
            "hzr_context_plan" => serde_json::json!({
                "intent":"trace ledger", "path":"crates", "topic":"architecture",
                "search_limit":1, "memory_limit":1
            }),
            "hzr_codec" => serde_json::json!({
                "content":"text", "fidelity":"exact", "risk":"low", "profile":"safe"
            }),
            "hzr_read" => serde_json::json!({
                "path":"src/lib.rs", "outline":false, "from":1, "to":1, "max_lines":1
            }),
            "hzr_write" => serde_json::json!({
                "operation":"patch", "path":"src/lib.rs", "old":"old", "new":"new",
                "cas":true
            }),
            "hzr_exec" => serde_json::json!({"command":"pwd", "timeout_ms":1}),
            "hzr_observability" | "hzr_doctor" => serde_json::json!({}),
            other => panic!("missing sample for {other}"),
        }
    }

    #[test]
    fn acceptance_gate_mcp_descriptions_are_unique_and_schema_is_bounded() {
        // The 13-tool typed inventory serializes to 23,776 bytes. Keep under 25 KiB so
        // capability coverage cannot silently recreate the former 64 KiB prompt budget.
        const MAX_TOOL_SCHEMA_BYTES: usize = 25 * 1024;

        let definitions = super::tool_definitions();
        let encoded = serde_json::to_vec(&serde_json::json!({"tools": &definitions}))
            .expect("complete tools/list result serializes");
        assert!(
            encoded.len() <= MAX_TOOL_SCHEMA_BYTES,
            "MCP tool schema is {} bytes; budget is {MAX_TOOL_SCHEMA_BYTES}",
            encoded.len()
        );
        let descriptions = definitions
            .iter()
            .map(|definition| {
                definition
                    .get("description")
                    .and_then(serde_json::Value::as_str)
                    .expect("every MCP tool has a description")
            })
            .collect::<Vec<_>>();
        assert_eq!(
            descriptions.iter().copied().collect::<HashSet<_>>().len(),
            descriptions.len(),
            "MCP tools must not duplicate top-level descriptions"
        );
    }
}
