use std::path::Path;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use reqwest::{Client, StatusCode, header};
use sha2::{Digest, Sha256};
use tokio::io::{AsyncSeekExt, AsyncWriteExt};

use super::MAX_ARCHIVE_BYTES;
use super::progress::Progress;

#[cfg(test)]
#[path = "download_tests.rs"]
mod tests;

const ATTEMPTS: u32 = 4;
const IDLE_TIMEOUT: Duration = Duration::from_secs(30);

pub(super) fn client() -> Result<Client> {
    client_builder(IDLE_TIMEOUT)
        .build()
        .context("failed to construct the archive download client")
}

fn client_builder(idle_timeout: Duration) -> reqwest::ClientBuilder {
    Client::builder()
        .https_only(true)
        .redirect(reqwest::redirect::Policy::limited(5))
        .user_agent(concat!("hzr/", env!("CARGO_PKG_VERSION")))
        .connect_timeout(Duration::from_secs(10))
        // A healthy transfer may take minutes; only a stalled read should time out.
        .read_timeout(idle_timeout)
}

pub(super) async fn archive(
    client: &Client,
    url: &str,
    path: &Path,
    progress: &Progress,
) -> Result<String> {
    let mut file = tokio::fs::File::create(path)
        .await
        .with_context(|| format!("failed to create {}", path.display()))?;
    let mut digest = Sha256::new();
    let mut downloaded = 0_u64;
    let mut total = None;

    let mut attempt = 1;
    loop {
        let transfer: Result<()> = async {
            let mut request = client.get(url).header(header::ACCEPT_ENCODING, "identity");
            if downloaded > 0 {
                request = request.header(header::RANGE, format!("bytes={downloaded}-"));
            }
            let mut response = request
                .send()
                .await
                .context("failed to connect to the release download")?;
            let status = response.status();
            if status == StatusCode::PARTIAL_CONTENT && downloaded > 0 {
                let range = response
                    .headers()
                    .get(header::CONTENT_RANGE)
                    .context("resumed download has no Content-Range")?
                    .to_str()
                    .context("invalid Content-Range header")?;
                let resumed_total = range_total(range, downloaded)?;
                if total.is_some_and(|previous| previous != resumed_total) {
                    bail!("release archive size changed while resuming");
                }
                total = Some(resumed_total);
                if response
                    .content_length()
                    .is_some_and(|length| length != resumed_total - downloaded)
                {
                    bail!("resumed archive length disagrees with Content-Range");
                }
            } else if status == StatusCode::OK {
                // Some download servers ignore Range. Restart rather than append duplicate bytes.
                if downloaded > 0 {
                    file.set_len(0)
                        .await
                        .context("failed to reset the staged archive")?;
                    file.rewind()
                        .await
                        .context("failed to rewind the staged archive")?;
                    downloaded = 0;
                    digest = Sha256::new();
                }
                total = response.content_length();
            } else {
                response
                    .error_for_status_ref()
                    .context("release archive request failed")?;
                bail!("unexpected archive response: HTTP {status}");
            }
            if total.is_some_and(|length| length > MAX_ARCHIVE_BYTES) {
                bail!("release archive exceeds the {MAX_ARCHIVE_BYTES}-byte limit");
            }
            progress.bytes(downloaded, total);
            while let Some(chunk) = response
                .chunk()
                .await
                .context("release download interrupted")?
            {
                let next = downloaded.saturating_add(chunk.len() as u64);
                if next > MAX_ARCHIVE_BYTES || total.is_some_and(|length| next > length) {
                    bail!("release archive exceeds its permitted length");
                }
                file.write_all(&chunk)
                    .await
                    .context("failed to write the staged archive")?;
                digest.update(&chunk);
                downloaded = next;
                progress.bytes(downloaded, total);
            }
            if total.is_some_and(|length| downloaded != length) {
                bail!("release archive ended before its declared length");
            }
            Ok(())
        }
        .await;

        match transfer {
            Ok(()) => {
                file.sync_all().await.context("failed to sync the staged archive")?;
                return Ok(format!("{:x}", digest.finalize()));
            }
            Err(error) if attempt < ATTEMPTS && retryable(&error) => {
                let delay = 1_u64 << (attempt - 1);
                progress.step(format!(
                    "Connection interrupted; retry {}/{} in {delay}s (resume at {:.1} MiB)",
                    attempt, ATTEMPTS - 1, downloaded as f64 / 1_048_576.0
                ));
                tokio::time::sleep(Duration::from_secs(delay)).await;
                progress.step("Downloading release");
                attempt += 1;
            }
            Err(error) => return Err(error).with_context(|| format!(
                "release download failed after {attempt} attempt(s), {:.1} MiB received; run `hzr update` to retry",
                downloaded as f64 / 1_048_576.0
            )),
        }
    }
}

fn retryable(error: &anyhow::Error) -> bool {
    error.downcast_ref::<reqwest::Error>().is_some_and(|error| {
        error.is_timeout()
            || error.is_connect()
            || error.is_body()
            || error.is_decode()
            || error.status().is_some_and(|status| {
                status.is_server_error()
                    || status == StatusCode::TOO_MANY_REQUESTS
                    || status == StatusCode::REQUEST_TIMEOUT
            })
    })
}

fn range_total(range: &str, offset: u64) -> Result<u64> {
    let (bounds, total) = range
        .strip_prefix("bytes ")
        .and_then(|value| value.split_once('/'))
        .context("invalid archive Content-Range")?;
    let (start, end) = bounds
        .split_once('-')
        .context("invalid archive range bounds")?;
    let (start, end, total) = (
        start.parse::<u64>()?,
        end.parse::<u64>()?,
        total.parse::<u64>()?,
    );
    if start != offset
        || end < start
        || end.checked_add(1) != Some(total)
        || total > MAX_ARCHIVE_BYTES
    {
        bail!("archive Content-Range does not match the requested remainder");
    }
    Ok(total)
}
