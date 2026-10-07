use crate::{Asset, Channel, Release, ReleaseCache, Result, fail, model::now};
use reqwest::blocking::Client;
use reqwest::header::{ETAG, IF_NONE_MATCH};
use sha2::{Digest, Sha256};
use std::{
    fs::OpenOptions,
    io::{Read, Write},
    path::Path,
    sync::atomic::{AtomicBool, Ordering},
    time::Duration,
};

pub trait Remote: Send {
    fn release(
        &self,
        id: &str,
        channel: Channel,
        cached: Option<&ReleaseCache>,
    ) -> Result<ReleaseCache>;
    fn checksum(&self, id: &str, asset: &Asset, release: &Release) -> Result<String>;
    fn download(
        &self,
        id: &str,
        asset: &Asset,
        destination: &Path,
        cancel: &AtomicBool,
        progress: &mut dyn FnMut(f32),
    ) -> Result<String>;
}

pub struct Github {
    client: Client,
}
impl Github {
    pub fn new() -> Result<Self> {
        Ok(Self {
            client: Client::builder()
                .user_agent("CraftLauncher/0.1.0")
                .connect_timeout(Duration::from_secs(15))
                .timeout(Duration::from_secs(600))
                .https_only(true)
                .build()?,
        })
    }
}

pub fn trusted_asset_url(id: &str, raw: &str) -> Result<()> {
    crate::app(id)?;
    let url = reqwest::Url::parse(raw).map_err(|_| fail("Invalid download URL."))?;
    if url.scheme() != "https"
        || url.host_str() != Some("github.com")
        || url.port().is_some()
        || !url.username().is_empty()
        || url.password().is_some()
        || !url
            .path()
            .starts_with(&format!("/storytold/{id}/releases/download/"))
    {
        return Err(fail(
            "Downloads must come from the official storytold release.",
        ));
    }
    Ok(())
}

pub fn asset_for<'a>(release: &'a Release, platform: &str, arch: &str) -> Option<&'a Asset> {
    if platform == "linux" || platform == "freebsd" {
        let cpu = match arch {
            "x86_64" => "x86_64",
            "aarch64" => "aarch64",
            _ => return None,
        };
        release
            .assets
            .iter()
            .find(|a| {
                platform == "linux" && a.name.ends_with(&format!("{platform}-{cpu}.AppImage"))
            })
            .or_else(|| {
                release
                    .assets
                    .iter()
                    .find(|a| a.name.ends_with(&format!("{platform}-{cpu}.tar.gz")))
            })
    } else if platform == "macos" {
        release
            .assets
            .iter()
            .find(|a| {
                a.name.ends_with(".app.tar.gz")
                    && (a.name.contains("universal") || a.name.contains(arch))
            })
            .or_else(|| {
                release.assets.iter().find(|a| {
                    a.name.ends_with(".dmg")
                        && (a.name.contains("universal") || a.name.contains(arch))
                })
            })
    } else if platform == "windows" {
        let cpu = match arch {
            "x86_64" => "x64",
            "x86" => "x86",
            "aarch64" => "arm64",
            _ => return None,
        };
        release
            .assets
            .iter()
            .find(|a| a.name.ends_with(&format!("windows-{cpu}-portable.zip")))
            .or_else(|| {
                release
                    .assets
                    .iter()
                    .find(|a| a.name.ends_with(&format!("{cpu}-setup.exe")))
            })
    } else {
        None
    }
}

impl Remote for Github {
    fn release(
        &self,
        id: &str,
        channel: Channel,
        cached: Option<&ReleaseCache>,
    ) -> Result<ReleaseCache> {
        crate::app(id)?;
        let mut request = self
            .client
            .get(format!(
                "https://api.github.com/repos/storytold/{id}/releases?per_page=30"
            ))
            .timeout(Duration::from_secs(25))
            .header("Accept", "application/vnd.github+json");
        if let Some(cache) = cached.filter(|c| c.channel == channel)
            && let Some(etag) = &cache.etag
        {
            request = request.header(IF_NONE_MATCH, etag);
        }
        let response = request.send()?;
        if response.status().as_u16() == 304 {
            let mut cache = cached
                .cloned()
                .ok_or_else(|| fail("Invalid GitHub cache response."))?;
            cache.checked_at = now();
            cache.error = None;
            return Ok(cache);
        }
        if matches!(response.status().as_u16(), 403 | 429) {
            return Err(fail("GitHub rate limit reached. Please try again later."));
        }
        let response = response.error_for_status()?;
        let etag = response
            .headers()
            .get(ETAG)
            .and_then(|v| v.to_str().ok())
            .map(str::to_owned);
        let mut releases: Vec<Release> = response.json()?;
        releases.retain(|r| !r.draft && (channel == Channel::Preview || !r.prerelease));
        releases.sort_by(|a, b| {
            crate::model::version_number(&b.tag_name)
                .cmp(&crate::model::version_number(&a.tag_name))
                .then_with(|| b.published_at.cmp(&a.published_at))
        });
        Ok(ReleaseCache {
            channel,
            etag,
            checked_at: now(),
            release: releases.first().cloned(),
            versions: releases,
            error: None,
        })
    }
    fn checksum(&self, id: &str, asset: &Asset, release: &Release) -> Result<String> {
        if let Some(digest) = asset
            .digest
            .as_deref()
            .and_then(|d| d.strip_prefix("sha256:"))
            && valid_hash(digest)
        {
            return Ok(digest.to_ascii_lowercase());
        }
        let sums = release
            .assets
            .iter()
            .find(|a| a.name == "SHA256SUMS.txt")
            .ok_or_else(|| {
                fail("This release has no SHA-256 checksum. Use the official download page.")
            })?;
        trusted_asset_url(id, &sums.browser_download_url)?;
        let mut response = self
            .client
            .get(&sums.browser_download_url)
            .timeout(Duration::from_secs(25))
            .send()?
            .error_for_status()?;
        let mut body = String::new();
        Read::by_ref(&mut response)
            .take(100_001)
            .read_to_string(&mut body)?;
        if body.len() > 100_000 {
            return Err(fail("Invalid checksum manifest."));
        }
        checksum_from_manifest(&body, &asset.name)
    }
    fn download(
        &self,
        id: &str,
        asset: &Asset,
        destination: &Path,
        cancel: &AtomicBool,
        progress: &mut dyn FnMut(f32),
    ) -> Result<String> {
        trusted_asset_url(id, &asset.browser_download_url)?;
        if asset.size == 0 || asset.size > 2 * 1024 * 1024 * 1024 {
            return Err(fail("Invalid download size."));
        }
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()?;
        runtime.block_on(async {
            let client = reqwest::Client::builder().user_agent("CraftLauncher/0.1.0").https_only(true)
                .connect_timeout(Duration::from_secs(15)).read_timeout(Duration::from_secs(20)).timeout(Duration::from_secs(600)).build()?;
            let mut response = tokio::select! {
                _ = wait_cancel(cancel) => return Err(fail("Download cancelled.")),
                response = client.get(&asset.browser_download_url).send() => response?.error_for_status()?,
            };
            let mut file = OpenOptions::new().create_new(true).write(true).open(destination)?;
            let mut hash = Sha256::new(); let mut bytes = 0u64; let mut last = -1;
            loop {
                let chunk = tokio::select! {
                    _ = wait_cancel(cancel) => return Err(fail("Download cancelled.")),
                    chunk = response.chunk() => chunk?,
                };
                let Some(chunk) = chunk else { break; };
                bytes += chunk.len() as u64;
                if bytes > asset.size { return Err(fail("Download exceeded its expected size.")); }
                file.write_all(&chunk)?; hash.update(&chunk);
                let percent = (bytes * 100 / asset.size) as i32;
                if percent != last { progress(percent as f32); last = percent; }
            }
            file.sync_all()?;
            if bytes != asset.size { return Err(fail("Incomplete download. Please try again.")); }
            Ok(format!("{:x}", hash.finalize()))
        })
    }
}

fn valid_hash(s: &str) -> bool {
    s.len() == 64 && s.bytes().all(|c| c.is_ascii_hexdigit())
}
pub fn checksum_from_manifest(body: &str, filename: &str) -> Result<String> {
    for line in body.lines() {
        let Some((hash, name)) = line.split_once(char::is_whitespace) else {
            continue;
        };
        if name.trim().trim_start_matches('*') == filename && valid_hash(hash) {
            return Ok(hash.to_ascii_lowercase());
        }
    }
    Err(fail("No checksum found for this download."))
}

async fn wait_cancel(cancel: &AtomicBool) {
    while !cancel.load(Ordering::Relaxed) {
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn release(names: &[&str]) -> Release {
        Release {
            tag_name: "v1.0.0".into(),
            name: None,
            body: None,
            published_at: None,
            prerelease: false,
            draft: false,
            assets: names
                .iter()
                .map(|name| Asset {
                    name: (*name).into(),
                    browser_download_url: String::new(),
                    size: 1,
                    digest: None,
                })
                .collect(),
        }
    }

    #[test]
    fn linux_prefers_appimage_regardless_of_asset_order_and_never_another_cpu() {
        for names in [
            [
                "app-linux-x86_64.tar.gz",
                "app-linux-aarch64.AppImage",
                "app-linux-x86_64.AppImage",
            ],
            [
                "app-linux-x86_64.AppImage",
                "app-linux-aarch64.AppImage",
                "app-linux-x86_64.tar.gz",
            ],
        ] {
            let release = release(&names);
            assert_eq!(
                asset_for(&release, "linux", "x86_64").unwrap().name,
                "app-linux-x86_64.AppImage"
            );
            assert_eq!(
                asset_for(&release, "linux", "aarch64").unwrap().name,
                "app-linux-aarch64.AppImage"
            );
            assert!(asset_for(&release, "linux", "x86").is_none());
        }
        let release = release(&["app-linux-aarch64.AppImage", "app-linux-x86_64.tar.gz"]);
        assert_eq!(
            asset_for(&release, "linux", "x86_64").unwrap().name,
            "app-linux-x86_64.tar.gz"
        );
    }

    #[test]
    fn incompatible_artcraft_release_does_not_offer_a_linux_install() {
        let release = release(&[
            "ArtCraft_0.41.0_universal.dmg",
            "ArtCraft_0.41.0_x64-setup.exe",
            "ArtCraft_0.41.0_x64_en-US.msi",
            "ArtCraft_universal.app.tar.gz",
        ]);
        assert!(asset_for(&release, "linux", "x86_64").is_none());
        assert!(asset_for(&release, "linux", "aarch64").is_none());
        assert!(asset_for(&release, "freebsd", "x86_64").is_none());
        assert_eq!(
            asset_for(&release, "macos", "aarch64").unwrap().name,
            "ArtCraft_universal.app.tar.gz"
        );
        assert_eq!(
            asset_for(&release, "windows", "x86_64").unwrap().name,
            "ArtCraft_0.41.0_x64-setup.exe"
        );
        assert!(asset_for(&release, "windows", "aarch64").is_none());
    }

    #[test]
    fn freebsd_uses_native_tar_and_rejects_appimage_and_unknown_platform() {
        let release = release(&[
            "app-linux-x86_64.AppImage",
            "app-freebsd-x86_64.AppImage",
            "app-freebsd-x86_64.tar.gz",
        ]);
        assert_eq!(
            asset_for(&release, "freebsd", "x86_64").unwrap().name,
            "app-freebsd-x86_64.tar.gz"
        );
        assert!(asset_for(&release, "freebsd", "aarch64").is_none());
        assert!(asset_for(&release, "android", "aarch64").is_none());
    }

    #[test]
    fn checksum_manifest_matches_exact_filename_and_normalizes_valid_hex() {
        let hash = "AB".repeat(32);
        let text = format!("invalid  app.tar.gz\n{hash}  *other-app.tar.gz\n{hash}\t*app.tar.gz\n");
        assert_eq!(
            checksum_from_manifest(&text, "app.tar.gz").unwrap(),
            "ab".repeat(32)
        );
        assert!(checksum_from_manifest(&text, "tar.gz").is_err());
        assert!(checksum_from_manifest(&text, "missing.tar.gz").is_err());
        assert!(
            checksum_from_manifest(&format!("{}  app.tar.gz", "G".repeat(64)), "app.tar.gz")
                .is_err()
        );
    }
}
