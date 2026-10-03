use crate::Progress;
use anyhow::{Context, Result, bail, ensure};
use reqwest::blocking::Client;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    fs::File,
    io::{Read, Write},
    path::Path,
    time::{Duration, Instant},
};

pub const MAX_DOWNLOAD: u64 = 8 * 1024 * 1024 * 1024;

pub fn client() -> Result<Client> {
    Ok(Client::builder()
        .user_agent("NeelemaNet-Launcher/0.1")
        .connect_timeout(Duration::from_secs(20))
        .timeout(Duration::from_secs(3600))
        .https_only(true)
        .build()?)
}

pub fn get_text(url: &str, limit: u64) -> Result<String> {
    let mut response = client()?
        .get(url)
        .timeout(Duration::from_secs(30))
        .send()?
        .error_for_status()?
        .take(limit + 1);
    let mut body = String::new();
    response.read_to_string(&mut body)?;
    ensure!(
        body.len() as u64 <= limit,
        "Response exceeds the size limit"
    );
    Ok(body)
}

pub fn copy_checked(
    mut input: impl Read,
    output: &Path,
    expected: Option<&str>,
    total: Option<u64>,
    progress: &Progress<'_>,
) -> Result<()> {
    if let Some(total) = total {
        ensure!(total <= MAX_DOWNLOAD, "Download exceeds 8 GiB limit");
    }
    let mut file = File::create(output)?;
    let mut hash = expected.map(|_| Sha256::new());
    let mut buffer = [0_u8; 128 * 1024];
    let mut bytes = 0_u64;
    let mut last = Instant::now();
    loop {
        let count = input
            .read(&mut buffer)
            .context("Download interrupted; please retry")?;
        if count == 0 {
            break;
        }
        bytes += count as u64;
        ensure!(bytes <= MAX_DOWNLOAD, "Download exceeds 8 GiB limit");
        file.write_all(&buffer[..count])?;
        if let Some(hash) = &mut hash {
            hash.update(&buffer[..count]);
        }
        if last.elapsed() >= Duration::from_millis(100) {
            progress(
                format!("Downloading · {:.1} MiB", bytes as f64 / 1048576.0),
                total.filter(|t| *t > 0).map(|t| bytes as f32 / t as f32),
            );
            last = Instant::now();
        }
    }
    if let Some(total) = total {
        ensure!(bytes == total, "Download was incomplete; please retry");
    }
    if let (Some(expected), Some(hash)) = (expected, hash) {
        let actual = format!("{:x}", hash.finalize());
        if !actual.eq_ignore_ascii_case(expected) {
            bail!("SHA-256 checksum mismatch. The download was not installed.");
        }
    }
    file.sync_all()?;
    Ok(())
}

/// HTTP validators are change indicators, not checksums of downloaded pack contents.
#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
pub struct DownloadStamp {
    pub etag: Option<String>,
    pub last_modified: Option<String>,
    pub length: Option<u64>,
}

impl DownloadStamp {
    pub fn from_response(response: &reqwest::blocking::Response) -> Self {
        let header = |name| {
            response
                .headers()
                .get(name)
                .and_then(|v| v.to_str().ok())
                .map(str::to_owned)
        };
        Self {
            etag: header(reqwest::header::ETAG),
            last_modified: header(reqwest::header::LAST_MODIFIED),
            // HEAD has no response body; read the object's length from the header itself.
            length: header(reqwest::header::CONTENT_LENGTH).and_then(|v| v.parse().ok()),
        }
    }

    pub fn differs_from(&self, old: &Self) -> bool {
        if let (Some(new), Some(old)) = (&self.etag, &old.etag) {
            return new != old;
        }
        if let (Some(new), Some(old)) = (&self.last_modified, &old.last_modified)
            && new != old
        {
            return true;
        }
        matches!((self.length, old.length), (Some(new), Some(old)) if new != old)
    }
}

pub fn pack_stamp(url: &str) -> Result<DownloadStamp> {
    let response = client()?
        .head(url)
        .timeout(Duration::from_secs(15))
        .send()?
        .error_for_status()?;
    Ok(DownloadStamp::from_response(&response))
}

pub fn download_pack(url: &str, output: &Path, progress: &Progress<'_>) -> Result<DownloadStamp> {
    let response = client()?.get(url).send()?.error_for_status()?;
    let stamp = DownloadStamp::from_response(&response);
    copy_checked(response, output, None, stamp.length, progress)?;
    Ok(stamp)
}

pub fn download(
    url: &str,
    output: &Path,
    expected: Option<&str>,
    progress: &Progress<'_>,
) -> Result<()> {
    let response = client()?
        .get(url)
        .send()
        .context("Could not download file")?
        .error_for_status()?;
    let length = response.content_length();
    copy_checked(response, output, expected, length, progress)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_replaced_download_even_when_size_is_unchanged() {
        let old = DownloadStamp {
            etag: Some("old-object".into()),
            length: Some(100),
            ..Default::default()
        };
        let new = DownloadStamp {
            etag: Some("new-object".into()),
            length: Some(100),
            ..Default::default()
        };
        assert!(new.differs_from(&old));
        assert!(!old.differs_from(&old));
    }

    #[test]
    fn uses_last_modified_and_length_when_etag_is_unavailable() {
        let old = DownloadStamp {
            last_modified: Some("yesterday".into()),
            length: Some(100),
            ..Default::default()
        };
        let changed_time = DownloadStamp {
            last_modified: Some("today".into()),
            ..old.clone()
        };
        let changed_size = DownloadStamp {
            length: Some(101),
            ..old.clone()
        };
        assert!(changed_time.differs_from(&old));
        assert!(changed_size.differs_from(&old));
        assert!(!DownloadStamp::default().differs_from(&old));
    }

    #[test]
    fn head_reads_object_length_even_though_body_is_empty() {
        use std::{net::TcpListener, thread};
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let server = thread::spawn(move || {
            let (mut socket, _) = listener.accept().unwrap();
            socket
                .set_read_timeout(Some(Duration::from_secs(5)))
                .unwrap();
            let mut request = Vec::new();
            let mut byte = [0];
            while !request.ends_with(b"\r\n\r\n") {
                socket.read_exact(&mut byte).unwrap();
                request.push(byte[0]);
            }
            assert!(request.starts_with(b"HEAD "));
            socket.write_all(b"HTTP/1.1 200 OK\r\nETag: \"revision-2\"\r\nContent-Length: 709401927\r\nConnection: close\r\n\r\n").unwrap();
        });
        // HTTP is confined to this loopback fixture; production downloads require HTTPS.
        let response = Client::builder()
            .no_proxy()
            .timeout(Duration::from_secs(5))
            .build()
            .unwrap()
            .head(format!("http://{address}/pack.zip"))
            .send()
            .unwrap();
        let stamp = DownloadStamp::from_response(&response);
        assert_eq!(stamp.length, Some(709401927));
        assert_eq!(stamp.etag.as_deref(), Some("\"revision-2\""));
        server.join().unwrap();
    }

    #[test]
    fn unfinished_download_still_fails_without_a_checksum() {
        let temp = tempfile::tempdir().unwrap();
        assert!(
            copy_checked(
                &b"short"[..],
                &temp.path().join("pack.zip"),
                None,
                Some(100),
                &|_, _| {}
            )
            .is_err()
        );
    }
}
