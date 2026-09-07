use super::*;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

async fn server(
    replies: Vec<(&'static str, &'static str, Duration)>,
) -> (String, tokio::task::JoinHandle<Vec<String>>) {
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind fixture");
    let url = format!("http://{}/archive", listener.local_addr().expect("address"));
    let task = tokio::spawn(async move {
        let mut requests = Vec::new();
        for (headers, body, pause) in replies {
            let (mut socket, _) = listener.accept().await.expect("accept");
            let mut request = Vec::new();
            while !request.ends_with(b"\r\n\r\n") {
                request.push(socket.read_u8().await.expect("request byte"));
            }
            requests.push(String::from_utf8(request).expect("ASCII request"));
            socket.write_all(headers.as_bytes()).await.expect("headers");
            for byte in body.bytes() {
                if socket.write_all(&[byte]).await.is_err() {
                    break;
                }
                tokio::time::sleep(pause).await;
            }
        }
        requests
    });
    (url, task)
}

fn test_client(idle: Duration) -> Client {
    client_builder(idle)
        .https_only(false)
        .build()
        .expect("fixture client")
}

#[tokio::test]
async fn truncated_body_resumes_and_hashes_the_complete_archive() {
    let (url, server) = server(vec![
            ("HTTP/1.1 200 OK\r\nContent-Length: 6\r\nConnection: close\r\n\r\n", "abc", Duration::ZERO),
            ("HTTP/1.1 206 Partial Content\r\nContent-Length: 3\r\nContent-Range: bytes 3-5/6\r\nConnection: close\r\n\r\n", "def", Duration::ZERO),
        ]).await;
    let temp = tempfile::tempdir().expect("staging");
    let path = temp.path().join("archive");
    let digest = archive(
        &test_client(Duration::from_secs(1)),
        &url,
        &path,
        &Progress::new(false),
    )
    .await
    .expect("resumed");
    assert_eq!(std::fs::read(path).expect("archive"), b"abcdef");
    assert_eq!(digest, format!("{:x}", Sha256::digest(b"abcdef")));
    let requests = server.await.expect("server");
    assert!(requests[1].contains("range: bytes=3-\r\n"));
}

#[tokio::test]
async fn ignored_range_restarts_without_duplicate_bytes() {
    let (url, server) = server(vec![
        (
            "HTTP/1.1 200 OK\r\nContent-Length: 6\r\nConnection: close\r\n\r\n",
            "abc",
            Duration::ZERO,
        ),
        (
            "HTTP/1.1 200 OK\r\nContent-Length: 6\r\nConnection: close\r\n\r\n",
            "abcdef",
            Duration::ZERO,
        ),
    ])
    .await;
    let temp = tempfile::tempdir().expect("staging");
    let path = temp.path().join("archive");
    let digest = archive(
        &test_client(Duration::from_secs(1)),
        &url,
        &path,
        &Progress::new(false),
    )
    .await
    .expect("restarted");
    assert_eq!(std::fs::read(path).expect("archive"), b"abcdef");
    assert_eq!(digest, format!("{:x}", Sha256::digest(b"abcdef")));
    assert_eq!(server.await.expect("server").len(), 2);
}

#[tokio::test]
async fn healthy_slow_transfer_outlives_the_read_timeout() {
    let (url, server) = server(vec![(
        "HTTP/1.1 200 OK\r\nContent-Length: 6\r\nConnection: close\r\n\r\n",
        "abcdef",
        Duration::from_millis(40),
    )])
    .await;
    let temp = tempfile::tempdir().expect("staging");
    let started = std::time::Instant::now();
    let digest = archive(
        &test_client(Duration::from_millis(150)),
        &url,
        &temp.path().join("archive"),
        &Progress::new(false),
    )
    .await
    .expect("healthy slow transfer");
    assert!(started.elapsed() > Duration::from_millis(150));
    assert_eq!(digest, format!("{:x}", Sha256::digest(b"abcdef")));
    assert_eq!(server.await.expect("server").len(), 1);
}

#[tokio::test]
async fn server_failure_retries_but_missing_asset_does_not() {
    for status in ["503 Service Unavailable", "404 Not Found"] {
        let header = if status.starts_with("503") {
            "HTTP/1.1 503 Service Unavailable\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
        } else {
            "HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
        };
        let mut replies = vec![(header, "", Duration::ZERO)];
        if status.starts_with("503") {
            replies.push((
                "HTTP/1.1 200 OK\r\nContent-Length: 3\r\nConnection: close\r\n\r\n",
                "abc",
                Duration::ZERO,
            ));
        }
        let expected_requests = replies.len();
        let (url, server) = server(replies).await;
        let temp = tempfile::tempdir().expect("staging");
        let result = archive(
            &test_client(Duration::from_secs(1)),
            &url,
            &temp.path().join("archive"),
            &Progress::new(false),
        )
        .await;
        assert_eq!(result.is_ok(), status.starts_with("503"));
        assert_eq!(server.await.expect("server").len(), expected_requests);
    }
}

#[test]
fn invalid_resume_ranges_are_rejected() {
    for range in [
        "bytes 2-5/6",
        "bytes 3-4/6",
        "bytes 3-5/*",
        "bytes 3-2/6",
        "bytes 3-5/9999999999",
    ] {
        assert!(range_total(range, 3).is_err(), "{range}");
    }
    assert_eq!(range_total("bytes 3-5/6", 3).expect("exact remainder"), 6);
}
