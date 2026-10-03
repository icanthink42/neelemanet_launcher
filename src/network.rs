use crate::Progress;
use anyhow::{Context, Result, bail, ensure};
use reqwest::blocking::Client;
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
    let mut hash = Sha256::new();
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
        hash.update(&buffer[..count]);
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
    if let Some(expected) = expected {
        let actual = format!("{:x}", hash.finalize());
        if !actual.eq_ignore_ascii_case(expected) {
            bail!("SHA-256 checksum mismatch. The download was not installed.");
        }
    }
    file.sync_all()?;
    Ok(())
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
