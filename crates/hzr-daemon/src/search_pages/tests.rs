use super::*;
use hzr_protocol::{SearchHit, SearchMode, SearchStrategy};

fn request() -> SearchApiRequest {
    SearchApiRequest {
        workspace: "/canonical/project".into(),
        query: "symbol".into(),
        path: Some("src".into()),
        limit: 2,
        mode: SearchMode::Exact,
        include_content: false,
        paginate: true,
        cursor: None,
    }
}
fn response() -> SearchApiResponse {
    SearchApiResponse {
        page: None,
        query: "symbol".into(),
        path: "src".into(),
        total_hits: 5,
        shown_hits: 5,
        scanned_files: 5,
        skipped_large: 0,
        skipped_binary: 0,
        hits: (0..5)
            .map(|index| SearchHit {
                path: format!("src/file{index}.rs"),
                score: 1.0,
                matched_lines: 1,
                snippets: Vec::new(),
            })
            .collect(),
        effective_mode: SearchMode::Exact,
        strategy: SearchStrategy::ForkRgaiBuiltin,
        fallback_code: None,
        index_generation: Some("original-generation".into()),
        fallback_reason: None,
        next_step: None,
    }
}

#[test]
fn cursor_pages_are_stable_replayable_scoped_and_expire_without_rescan() {
    let directory = tempfile::tempdir().expect("directory");
    let mut request = request();
    let first = publish_at(directory.path(), &request, response(), 1000).expect("first");
    assert_eq!(first.hits[0].path, "src/file0.rs");
    let page = first.page.as_ref().expect("page");
    request.cursor = page.next_cursor.clone();
    let second = read_at(directory.path(), &request, 2000).expect("second");
    assert_eq!(second.hits[0].path, "src/file2.rs");
    assert_eq!(
        second,
        read_at(directory.path(), &request, 2000).expect("explicit replay")
    );
    assert_eq!(
        second.index_generation.as_deref(),
        Some("original-generation")
    );
    for (field, value) in [
        ("workspace", "/foreign"),
        ("query", "different"),
        ("path", "other"),
    ] {
        let mut wrong = request.clone();
        match field {
            "workspace" => wrong.workspace = value.into(),
            "query" => wrong.query = value.into(),
            _ => wrong.path = Some(value.into()),
        }
        assert!(read_at(directory.path(), &wrong, 2000).is_err());
    }
    let mut wrong = request.clone();
    wrong.mode = SearchMode::Semantic;
    assert!(read_at(directory.path(), &wrong, 2000).is_err());
    wrong = request.clone();
    wrong.include_content = true;
    assert!(read_at(directory.path(), &wrong, 2000).is_err());
    wrong = request.clone();
    wrong.limit = 3;
    assert!(read_at(directory.path(), &wrong, 2000).is_err());
    request.cursor = second.page.as_ref().expect("page").next_cursor.clone();
    let last = read_at(directory.path(), &request, 2000).expect("last");
    assert_eq!(last.hits.len(), 1);
    assert!(last.page.as_ref().expect("page").next_cursor.is_none());
    assert!(last.page.as_ref().expect("page").snapshot_complete);
    assert!(read_at(directory.path(), &request, 1000 + TTL_MS).is_err());
    let missing = tempfile::tempdir().expect("missing");
    assert!(read_at(missing.path(), &request, 2000).is_err());
    assert!(!super::directory(missing.path()).exists());
}

#[test]
fn cursor_rejects_malformed_paths_and_reports_snapshot_truncation() {
    let directory = tempfile::tempdir().expect("directory");
    let mut request = request();
    request.cursor = Some("../../secret:0".into());
    assert!(read_at(directory.path(), &request, 1000).is_err());
    request.cursor = None;
    request.limit = 5;
    let mut all = response();
    all.total_hits = 200;
    let first = publish_at(directory.path(), &request, all, 1000).expect("bounded snapshot");
    let page = first.page.expect("page");
    assert!(!page.snapshot_complete);
    assert!(page.next_cursor.is_none());
    assert!(first.next_step.expect("scope recovery").contains("narrow"));
}

#[cfg(unix)]
#[test]
fn snapshot_is_private_and_symlink_payload_is_rejected() {
    use std::os::unix::fs::{PermissionsExt, symlink};
    let directory = tempfile::tempdir().expect("directory");
    let mut request = request();
    let first = publish_at(directory.path(), &request, response(), 1000).expect("first");
    let page = first.page.expect("page");
    let path = super::directory(directory.path()).join(format!("{}.json", page.snapshot_id));
    assert_eq!(
        fs::metadata(&path).expect("metadata").permissions().mode() & 0o777,
        0o600
    );
    request.cursor = page.next_cursor;
    let outside = directory.path().join("outside.json");
    fs::rename(&path, &outside).expect("move fixture");
    symlink(outside, &path).expect("symlink");
    assert!(read_at(directory.path(), &request, 2000).is_err());
}
