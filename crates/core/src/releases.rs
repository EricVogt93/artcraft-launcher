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
    fn discover(&self, _cancel: &AtomicBool) -> Result<Vec<crate::workspace::DiscoveredApp>> {
        Ok(Vec::new())
    }
    fn discover_cached(
        &self,
        cancel: &AtomicBool,
        cached: &[crate::workspace::DiscoveryPage],
    ) -> Result<(
        Vec<crate::workspace::DiscoveredApp>,
        Vec<crate::workspace::DiscoveryPage>,
    )> {
        Ok((self.discover(cancel)?, cached.to_vec()))
    }
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
    fn release_product(
        &self,
        product: &crate::catalog::Product,
        channel: Channel,
        cached: Option<&ReleaseCache>,
    ) -> Result<ReleaseCache> {
        self.release(&product.id, channel, cached)
    }
    fn checksum_product(
        &self,
        product: &crate::catalog::Product,
        asset: &Asset,
        release: &Release,
    ) -> Result<String> {
        self.checksum(&product.id, asset, release)
    }
    fn download_product(
        &self,
        product: &crate::catalog::Product,
        asset: &Asset,
        destination: &Path,
        cancel: &AtomicBool,
        progress: &mut dyn FnMut(f32),
    ) -> Result<String> {
        self.download(&product.id, asset, destination, cancel, progress)
    }
}

pub struct Github {
    client: Client,
    async_client: reqwest::Client,
    runtime: tokio::runtime::Runtime,
    token: Option<String>,
}
impl Github {
    pub fn new() -> Result<Self> {
        let token = ["CRAFTLAUNCHER_GITHUB_TOKEN", "GITHUB_TOKEN"]
            .into_iter()
            .find_map(|key| std::env::var(key).ok())
            .map(|value| value.trim().to_owned())
            .filter(|value| !value.is_empty());
        Ok(Self {
            client: Client::builder()
                .user_agent("CraftLauncher/0.1.0")
                .connect_timeout(Duration::from_secs(15))
                .timeout(Duration::from_secs(600))
                .https_only(true)
                .build()?,
            async_client: reqwest::Client::builder()
                .user_agent("CraftLauncher/0.1.0")
                .https_only(true)
                .connect_timeout(Duration::from_secs(15))
                .read_timeout(Duration::from_secs(20))
                .timeout(Duration::from_secs(600))
                .build()?,
            runtime: tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()?,
            token,
        })
    }
}

pub fn trusted_asset_url(id: &str, raw: &str) -> Result<()> {
    trusted_repository_asset_url(crate::app(id)?.repository(), raw)
}

pub fn trusted_repository_asset_url(repository: &str, raw: &str) -> Result<()> {
    if repository.is_empty()
        || !repository
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || c == b'-')
    {
        return Err(fail("Invalid product repository."));
    }
    let url = reqwest::Url::parse(raw).map_err(|_| fail("Invalid download URL."))?;
    if url.scheme() != "https"
        || url.host_str() != Some("github.com")
        || url.port().is_some()
        || !url.username().is_empty()
        || url.password().is_some()
        || !url
            .path()
            .starts_with(&format!("/storytold/{repository}/releases/download/"))
    {
        return Err(fail(
            "Downloads must come from the official storytold release.",
        ));
    }
    Ok(())
}

/// Select only packages whose platform and CPU are unambiguous. The product
/// name may contain separators, so compare complete tokens rather than substrings.
#[derive(Debug)]
pub enum AssetResolution<'a> {
    Ready(&'a Asset),
    Ambiguous(Vec<&'a Asset>),
    NoCompatible,
}
impl<'a> AssetResolution<'a> {
    pub fn selected(self) -> Option<&'a Asset> {
        match self {
            Self::Ready(asset) => Some(asset),
            _ => None,
        }
    }
}
pub fn asset_preference_key(platform: &str, arch: &str) -> String {
    format!("{platform}/{arch}")
}
pub fn asset_for<'a>(release: &'a Release, platform: &str, arch: &str) -> Option<&'a Asset> {
    resolve_asset(release, platform, arch, None).selected()
}
pub fn resolve_asset<'a>(
    release: &'a Release,
    platform: &str,
    arch: &str,
    preferred: Option<&str>,
) -> AssetResolution<'a> {
    fn tokens(name: &str) -> Vec<&str> {
        name.split(|c: char| !c.is_ascii_alphanumeric())
            .filter(|part| !part.is_empty())
            .collect()
    }
    let cpus: &[&str] = match arch {
        "x86_64" => &["x8664", "amd64", "x64"],
        "aarch64" => &["aarch64", "arm64"],
        "x86" => &["x86", "i686", "i386"],
        _ => return AssetResolution::NoCompatible,
    };
    let platforms: &[&str] = match platform {
        "linux" => &["linux"],
        "freebsd" => &["freebsd"],
        "macos" => &["macos", "darwin", "osx"],
        "windows" => &["windows", "win"],
        _ => return AssetResolution::NoCompatible,
    };
    let mut matches = Vec::new();
    for asset in &release.assets {
        let name = asset.name.to_ascii_lowercase().replace("x86_64", "x8664");
        let parts = tokens(&name);
        // macOS .app.tar.gz / .app.zip releases have historically omitted the OS token.
        let os_match = parts.iter().any(|p| platforms.contains(p))
            || (platform == "macos"
                && (name.ends_with(".app.tar.gz")
                    || name.ends_with(".app.zip")
                    || name.ends_with(".dmg")))
            || (platform == "windows" && name.ends_with("-setup.exe"));
        let cpu_match = parts.iter().any(|p| cpus.contains(p))
            || (platform == "macos" && parts.contains(&"universal"));
        if !os_match || !cpu_match {
            continue;
        }
        let priority = match platform {
            "linux" if name.ends_with(".appimage") => 0,
            "linux" | "freebsd" if name.ends_with(".tar.gz") || name.ends_with(".tgz") => 1,
            "macos" if name.ends_with(".app.tar.gz") => 0,
            "macos" if name.ends_with(".dmg") => 1,
            "macos" if name.ends_with(".zip") => 2,
            "windows" if name.ends_with("-portable.zip") => 0,
            "windows" if name.ends_with(".zip") => 1,
            "windows" if name.ends_with("-setup.exe") => 2,
            _ => continue,
        };
        matches.push((priority, asset));
    }
    matches.sort_by_key(|(priority, _)| *priority);
    let Some((priority, _)) = matches.first().copied() else {
        return AssetResolution::NoCompatible;
    };
    let best: Vec<&Asset> = matches
        .into_iter()
        .take_while(|(p, _)| *p == priority)
        .map(|(_, a)| a)
        .collect();
    if let Some(chosen) = preferred.and_then(|name| best.iter().find(|a| a.name == name)) {
        return AssetResolution::Ready(chosen);
    }
    if best.len() == 1 {
        AssetResolution::Ready(best[0])
    } else {
        AssetResolution::Ambiguous(best)
    }
}

/// The digest supplied by GitHub is preferred; otherwise accept the common
/// global manifest or a manifest named for this exact package stem.
#[derive(Debug)]
pub enum ChecksumSource<'a> {
    Digest(&'a str),
    Manifest(&'a Asset),
    Missing,
}
pub fn checksum_source<'a>(release: &'a Release, asset: &'a Asset) -> ChecksumSource<'a> {
    if let Some(digest) = asset
        .digest
        .as_deref()
        .and_then(|d| d.strip_prefix("sha256:"))
        && valid_hash(digest)
    {
        return ChecksumSource::Digest(digest);
    }
    let lower = asset.name.to_ascii_lowercase();
    let stem = [
        ".app.tar.gz",
        ".app.zip",
        ".tar.gz",
        ".tgz",
        ".appimage",
        ".zip",
        ".dmg",
        ".exe",
        ".deb",
        ".rpm",
    ]
    .into_iter()
    .find_map(|suffix| lower.strip_suffix(suffix))
    .unwrap_or(&lower);
    let expected = format!("{stem}-sha256sums.txt");
    let manifest = release
        .assets
        .iter()
        .find(|item| item.name.eq_ignore_ascii_case(&expected))
        .or_else(|| {
            release
                .assets
                .iter()
                .find(|item| item.name.eq_ignore_ascii_case("SHA256SUMS.txt"))
        });
    manifest.map_or(ChecksumSource::Missing, ChecksumSource::Manifest)
}

impl Remote for Github {
    fn discover(&self, cancel: &AtomicBool) -> Result<Vec<crate::workspace::DiscoveredApp>> {
        Ok(self.discover_cached(cancel, &[])?.0)
    }
    fn discover_cached(
        &self,
        cancel: &AtomicBool,
        cached: &[crate::workspace::DiscoveryPage],
    ) -> Result<(
        Vec<crate::workspace::DiscoveredApp>,
        Vec<crate::workspace::DiscoveryPage>,
    )> {
        discover_pages_with_token(
            &self.client,
            "https://api.github.com",
            self.token.as_deref(),
            cached,
            cancel,
        )
    }
    fn release(
        &self,
        id: &str,
        channel: Channel,
        cached: Option<&ReleaseCache>,
    ) -> Result<ReleaseCache> {
        self.release_product(
            &crate::catalog::Product::from(crate::app(id)?),
            channel,
            cached,
        )
    }
    fn release_product(
        &self,
        product: &crate::catalog::Product,
        channel: Channel,
        cached: Option<&ReleaseCache>,
    ) -> Result<ReleaseCache> {
        fetch_release_product(
            &self.client,
            "https://api.github.com",
            self.token.as_deref(),
            &product.repository,
            channel,
            cached,
        )
    }
    fn checksum(&self, id: &str, asset: &Asset, release: &Release) -> Result<String> {
        self.checksum_product(
            &crate::catalog::Product::from(crate::app(id)?),
            asset,
            release,
        )
    }
    fn checksum_product(
        &self,
        product: &crate::catalog::Product,
        asset: &Asset,
        release: &Release,
    ) -> Result<String> {
        let sums = match checksum_source(release, asset) {
            ChecksumSource::Digest(digest) => return Ok(digest.to_ascii_lowercase()),
            ChecksumSource::Manifest(sums) => sums,
            ChecksumSource::Missing => {
                return Err(fail(
                    "This release has no SHA-256 checksum. Use the official download page.",
                ));
            }
        };
        trusted_repository_asset_url(&product.repository, &sums.browser_download_url)?;
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
        self.download_product(
            &crate::catalog::Product::from(crate::app(id)?),
            asset,
            destination,
            cancel,
            progress,
        )
    }
    fn download_product(
        &self,
        product: &crate::catalog::Product,
        asset: &Asset,
        destination: &Path,
        cancel: &AtomicBool,
        progress: &mut dyn FnMut(f32),
    ) -> Result<String> {
        trusted_repository_asset_url(&product.repository, &asset.browser_download_url)?;
        if asset.size == 0 || asset.size > 2 * 1024 * 1024 * 1024 {
            return Err(fail("Invalid download size."));
        }
        self.runtime.block_on(download_http(
            &self.async_client,
            &asset.browser_download_url,
            asset.size,
            destination,
            cancel,
            progress,
        ))
    }
}

fn fetch_release_product(
    client: &Client,
    base: &str,
    token: Option<&str>,
    repository: &str,
    channel: Channel,
    cached: Option<&ReleaseCache>,
) -> Result<ReleaseCache> {
    let mut request = client
        .get(format!(
            "{base}/repos/storytold/{repository}/releases?per_page=30"
        ))
        .timeout(Duration::from_secs(25))
        .header("Accept", "application/vnd.github+json");
    if let Some(token) = token {
        request = request.bearer_auth(token);
    }
    if let Some(cache) = cached
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
        cache.set_channel(channel);
        return Ok(cache);
    }
    if matches!(response.status().as_u16(), 403 | 429) {
        return Err(crate::Error::RateLimited);
    }
    let response = response.error_for_status()?;
    let etag = response
        .headers()
        .get(ETAG)
        .and_then(|v| v.to_str().ok())
        .map(str::to_owned);
    let mut releases: Vec<Release> = response.json()?;
    releases.retain(|r| !r.draft);
    releases.sort_by(|a, b| {
        crate::model::version_number(&b.tag_name)
            .cmp(&crate::model::version_number(&a.tag_name))
            .then_with(|| b.published_at.cmp(&a.published_at))
    });
    let mut cache = ReleaseCache {
        channel,
        etag,
        checked_at: now(),
        release: None,
        versions: releases,
        error: None,
    };
    cache.set_channel(channel);
    Ok(cache)
}

#[cfg(test)]
fn discover_pages(
    client: &Client,
    base: &str,
    cached: &[crate::workspace::DiscoveryPage],
    cancel: &AtomicBool,
) -> Result<(
    Vec<crate::workspace::DiscoveredApp>,
    Vec<crate::workspace::DiscoveryPage>,
)> {
    discover_pages_with_token(client, base, None, cached, cancel)
}

fn discover_pages_with_token(
    client: &Client,
    base: &str,
    token: Option<&str>,
    cached: &[crate::workspace::DiscoveryPage],
    cancel: &AtomicBool,
) -> Result<(
    Vec<crate::workspace::DiscoveredApp>,
    Vec<crate::workspace::DiscoveryPage>,
)> {
    let mut found = Vec::new();
    let mut pages = Vec::new();
    for number in 1..=10 {
        if cancel.load(Ordering::Relaxed) {
            return Err(crate::cancelled("Discovery cancelled."));
        }
        let previous = cached.get(number - 1);
        let mut request = client
            .get(format!(
                "{base}/users/storytold/repos?per_page=100&page={number}"
            ))
            .timeout(Duration::from_secs(25))
            .header("Accept", "application/vnd.github+json");
        if let Some(token) = token {
            request = request.bearer_auth(token);
        }
        if let Some(etag) = previous.and_then(|p| p.etag.as_deref()) {
            request = request.header(IF_NONE_MATCH, etag);
        }
        let response = request.send()?;
        let page = if response.status() == reqwest::StatusCode::NOT_MODIFIED {
            previous
                .cloned()
                .ok_or_else(|| fail("GitHub returned an invalid discovery cache response."))?
        } else {
            if matches!(response.status().as_u16(), 403 | 429) {
                return Err(crate::Error::RateLimited);
            }
            let response = response.error_for_status()?;
            let etag = response
                .headers()
                .get(ETAG)
                .and_then(|v| v.to_str().ok())
                .map(str::to_owned);
            let repositories: Vec<Repository> = response.json()?;
            let last = repositories.len() < 100;
            crate::workspace::DiscoveryPage {
                etag,
                apps: discovery_candidates(repositories),
                last,
            }
        };
        found.extend(page.apps.iter().cloned());
        let last = page.last;
        pages.push(page);
        if last {
            found.sort_by_key(|app| app.repository.to_ascii_lowercase());
            return Ok((found, pages));
        }
    }
    Err(fail("Product discovery exceeded its page limit."))
}

async fn download_http(
    client: &reqwest::Client,
    url: &str,
    size: u64,
    destination: &Path,
    cancel: &AtomicBool,
    progress: &mut dyn FnMut(f32),
) -> Result<String> {
    let mut bytes = match destination.symlink_metadata() {
        Ok(meta) if meta.file_type().is_file() && meta.len() <= size => meta.len(),
        Ok(_) => return Err(fail("Invalid partial download file.")),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => 0,
        Err(error) => return Err(error.into()),
    };
    let mut hash = Sha256::new();
    if bytes > 0 {
        let mut existing = std::fs::File::open(destination)?;
        std::io::copy(&mut existing, &mut hash)?;
    }
    if bytes == size {
        progress(100.0);
        return Ok(format!("{:x}", hash.finalize()));
    }
    let mut request = client.get(url);
    if bytes > 0 {
        request = request.header(reqwest::header::RANGE, format!("bytes={bytes}-"));
    }
    let mut response = tokio::select! {
        _ = wait_cancel(cancel) => return Err(crate::cancelled("Download cancelled.")),
        response = request.send() => response?.error_for_status()?,
    };
    let append = if bytes > 0 && response.status() == reqwest::StatusCode::PARTIAL_CONTENT {
        let content_range = response
            .headers()
            .get(reqwest::header::CONTENT_RANGE)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("");
        let expected_prefix = format!("bytes {bytes}-");
        let Some(rest) = content_range.strip_prefix(&expected_prefix) else {
            return Err(fail("Invalid download range response."));
        };
        let Some((end, total)) = rest.split_once('/') else {
            return Err(fail("Invalid download range response."));
        };
        if end.parse::<u64>().ok() != size.checked_sub(1) || total.parse::<u64>().ok() != Some(size)
        {
            return Err(fail("Invalid download range response."));
        }
        true
    } else if response.status() == reqwest::StatusCode::OK {
        bytes = 0;
        hash = Sha256::new();
        false
    } else {
        return Err(fail("Unexpected download response."));
    };
    let mut file = OpenOptions::new()
        .create(true)
        .write(true)
        .append(append)
        .truncate(!append)
        .open(destination)?;
    progress((bytes as f64 * 100.0 / size as f64) as f32);
    let mut last = -1;
    loop {
        let chunk = tokio::select! {
            _ = wait_cancel(cancel) => return Err(crate::cancelled("Download cancelled.")),
            chunk = response.chunk() => chunk?,
        };
        let Some(chunk) = chunk else {
            break;
        };
        bytes += chunk.len() as u64;
        if bytes > size {
            return Err(fail("Download exceeded its expected size."));
        }
        file.write_all(&chunk)?;
        hash.update(&chunk);
        let percent = (bytes * 100 / size) as i32;
        if percent != last {
            progress((bytes as f64 * 100.0 / size as f64) as f32);
            last = percent;
        }
    }
    file.sync_all()?;
    if bytes != size {
        return Err(fail("Incomplete download. Please try again."));
    }
    Ok(format!("{:x}", hash.finalize()))
}

#[derive(serde::Deserialize)]
struct Repository {
    #[serde(default)]
    id: Option<u64>,
    name: String,
    full_name: String,
    description: Option<String>,
    fork: bool,
    archived: bool,
    disabled: bool,
    #[serde(default)]
    topics: Vec<String>,
}
fn discovery_candidates(repos: Vec<Repository>) -> Vec<crate::workspace::DiscoveredApp> {
    repos
        .into_iter()
        .filter(|repo| {
            let name = repo.name.to_ascii_lowercase();
            let topic_match = repo.topics.iter().any(|t| {
                matches!(
                    t.to_ascii_lowercase().as_str(),
                    "craft-app" | "artcraft-suite" | "craftlauncher-app"
                )
            });
            !repo.fork
                && !repo.archived
                && !repo.disabled
                && !matches!(name.as_str(), "craftlauncher" | "artcraft-launcher")
                && (name.ends_with("craft") || topic_match)
                && repo
                    .name
                    .bytes()
                    .all(|c| c.is_ascii_alphanumeric() || c == b'-')
                && repo.full_name == format!("storytold/{}", repo.name)
                && !crate::CATALOG
                    .iter()
                    .any(|app| app.repository().eq_ignore_ascii_case(&repo.name))
        })
        .map(|repo| crate::workspace::DiscoveredApp {
            app_id: repo.name.to_ascii_lowercase(),
            github_id: repo.id,
            unavailable: false,
            repository: repo.name,
            description: repo
                .description
                .unwrap_or_default()
                .chars()
                .take(400)
                .collect(),
        })
        .collect()
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

    fn serve_once(response: impl Into<String>) -> (String, std::thread::JoinHandle<String>) {
        use std::io::{Read, Write};
        let response = response.into();
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let handle = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            stream
                .set_read_timeout(Some(std::time::Duration::from_secs(3)))
                .unwrap();
            let mut request = [0; 4096];
            let len = stream.read(&mut request).unwrap();
            stream.write_all(response.as_bytes()).unwrap();
            String::from_utf8_lossy(&request[..len]).into_owned()
        });
        (format!("http://{address}/file"), handle)
    }
    #[test]
    fn discovery_reuses_etag_cache_on_not_modified_and_rejects_uncached_304() {
        let body = r#"[{"id":42,"name":"NewCraft","full_name":"storytold/NewCraft","description":"new","fork":false,"archived":false,"disabled":false}]"#;
        let response = format!(
            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nETag: \"v1\"\r\nConnection: close\r\n\r\n{}",
            body.len(),
            body
        );
        let client = Client::new();
        let (url, server) = serve_once(response);
        let (first, cache) = discover_pages(&client, &url, &[], &AtomicBool::new(false)).unwrap();
        assert_eq!(first[0].github_id, Some(42));
        assert_eq!(cache[0].etag.as_deref(), Some("\"v1\""));
        assert!(
            !server
                .join()
                .unwrap()
                .to_ascii_lowercase()
                .contains("if-none-match")
        );
        let response = "HTTP/1.1 304 Not Modified\r\nConnection: close\r\n\r\n";
        let (url, server) = serve_once(response);
        let (second, new_cache) =
            discover_pages(&client, &url, &cache, &AtomicBool::new(false)).unwrap();
        assert_eq!(second[0].repository, "NewCraft");
        assert_eq!(new_cache[0].etag, cache[0].etag);
        assert!(
            server
                .join()
                .unwrap()
                .to_ascii_lowercase()
                .contains("if-none-match: \"v1\"")
        );
        let (url, server) = serve_once(response);
        assert!(discover_pages(&client, &url, &[], &AtomicBool::new(false)).is_err());
        server.join().unwrap();
    }

    #[test]
    fn discovery_rejects_incomplete_pagination_instead_of_returning_partial_results() {
        use std::io::{Read, Write};
        let repos: Vec<_> = (0..100).map(|id| serde_json::json!({"id": id, "name": format!("repo{id}"), "full_name": format!("storytold/repo{id}"), "description": null, "fork": false, "archived": false, "disabled": false})).collect();
        let body = serde_json::to_string(&repos).unwrap();
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let server = std::thread::spawn(move || {
            for page in 1..=2 {
                let (mut stream, _) = listener.accept().unwrap();
                let mut request = [0; 4096];
                let _ = stream.read(&mut request).unwrap();
                let response = if page == 1 {
                    format!(
                        "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                        body.len(),
                        body
                    )
                } else {
                    "HTTP/1.1 500 Internal Server Error\r\nContent-Length: 0\r\nConnection: close\r\n\r\n".into()
                };
                stream.write_all(response.as_bytes()).unwrap();
            }
        });
        let result = discover_pages(
            &Client::new(),
            &format!("http://{address}"),
            &[],
            &AtomicBool::new(false),
        );
        assert!(result.is_err());
        server.join().unwrap();
    }

    #[test]
    fn partial_download_checks_range_and_restarts_when_server_ignores_it() {
        use sha2::Digest;
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("payload.part");
        let client = reqwest::Client::new();
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        for response in [
            "HTTP/1.1 206 Partial Content\r\nContent-Length: 3\r\nContent-Range: bytes 3-5/6\r\nConnection: close\r\n\r\ndef",
            "HTTP/1.1 200 OK\r\nContent-Length: 6\r\nConnection: close\r\n\r\nabcdef",
        ] {
            std::fs::write(&path, b"abc").unwrap();
            let (url, server) = serve_once(response);
            let hash = rt
                .block_on(download_http(
                    &client,
                    &url,
                    6,
                    &path,
                    &AtomicBool::new(false),
                    &mut |_| {},
                ))
                .unwrap();
            assert_eq!(hash, format!("{:x}", Sha256::digest(b"abcdef")));
            assert_eq!(std::fs::read(&path).unwrap(), b"abcdef");
            assert!(
                server
                    .join()
                    .unwrap()
                    .to_ascii_lowercase()
                    .contains("range: bytes=3-")
            );
        }
        std::fs::write(&path, b"abc").unwrap();
        let (url, server) = serve_once(
            "HTTP/1.1 206 Partial Content\r\nContent-Length: 3\r\nContent-Range: bytes 2-4/6\r\nConnection: close\r\n\r\ndef",
        );
        assert!(
            rt.block_on(download_http(
                &client,
                &url,
                6,
                &path,
                &AtomicBool::new(false),
                &mut |_| {}
            ))
            .is_err()
        );
        assert_eq!(std::fs::read(&path).unwrap(), b"abc");
        server.join().unwrap();
    }

    #[test]
    fn discovery_excludes_known_apps_forks_archives_and_untrusted_owners() {
        let repos: Vec<Repository> = serde_json::from_value(serde_json::json!([
            {"name":"NewCraft","full_name":"storytold/NewCraft","description":"New app","fork":false,"archived":false,"disabled":false},
            {"name":"pdfcraft","full_name":"storytold/pdfcraft","description":null,"fork":false,"archived":false,"disabled":false},
            {"name":"AniCraft","full_name":"storytold/AniCraft","description":null,"fork":false,"archived":false,"disabled":false},
            {"name":"oldcraft","full_name":"storytold/oldcraft","description":null,"fork":false,"archived":true,"disabled":false},
            {"name":"forkcraft","full_name":"storytold/forkcraft","description":null,"fork":true,"archived":false,"disabled":false},
            {"name":"evilcraft","full_name":"other/evilcraft","description":null,"fork":false,"archived":false,"disabled":false},
            {"name":"craft-libs","full_name":"storytold/craft-libs","description":null,"fork":false,"archived":false,"disabled":false},
            {"name":"craftlauncher","full_name":"storytold/craftlauncher","description":null,"fork":false,"archived":false,"disabled":false,"topics":["craft-app"]},
            {"name":"StudioFlow","full_name":"storytold/StudioFlow","description":"Node compositor","fork":false,"archived":false,"disabled":false,"topics":["artcraft-suite"]}
        ])).unwrap();
        let candidates = discovery_candidates(repos);
        assert_eq!(candidates.len(), 2);
        assert_eq!(candidates[0].repository, "NewCraft");
        assert_eq!(candidates[1].repository, "StudioFlow");
    }

    #[test]
    fn renamed_pdfcraft_downloads_remain_bound_to_the_official_repository() {
        assert!(
            trusted_asset_url(
                "printcraft",
                "https://github.com/storytold/pdfcraft/releases/download/v0.9.0/pdfcraft.zip"
            )
            .is_ok()
        );
        for url in [
            "https://github.com/storytold/wordcraft/releases/download/v1/app.zip",
            "https://github.com/other/pdfcraft/releases/download/v1/app.zip",
            "https://github.com/storytold/pdfcraft-extra/releases/download/v1/app.zip",
            "https://evil.test/storytold/pdfcraft/releases/download/v1/app.zip",
        ] {
            assert!(trusted_asset_url("printcraft", url).is_err());
        }
    }

    #[test]
    fn discovered_repository_downloads_are_bound_to_its_exact_owner_and_name() {
        assert!(
            trusted_repository_asset_url(
                "NewCraft",
                "https://github.com/storytold/NewCraft/releases/download/v1/a.tar.gz"
            )
            .is_ok()
        );
        for url in [
            "https://github.com/storytold/NewCraft-extra/releases/download/v1/a.tar.gz",
            "https://github.com/other/NewCraft/releases/download/v1/a.tar.gz",
            "https://evil.test/storytold/NewCraft/releases/download/v1/a.tar.gz",
        ] {
            assert!(trusted_repository_asset_url("NewCraft", url).is_err());
        }
        assert!(
            trusted_repository_asset_url(
                "../NewCraft",
                "https://github.com/storytold/NewCraft/releases/download/v1/a.tar.gz"
            )
            .is_err()
        );
    }

    #[test]
    fn every_catalog_app_has_a_trusted_release_source() {
        for app in crate::CATALOG {
            assert!(
                trusted_asset_url(
                    app.id,
                    &format!(
                        "https://github.com/storytold/{}/releases/download/v1/app.zip",
                        app.repository()
                    )
                )
                .is_ok()
            );
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
    fn recognizes_cpu_aliases_without_guessing_between_equal_packages() {
        let one = release(&[
            "newcraft-linux-amd64.AppImage",
            "newcraft-linux-arm64.tar.gz",
        ]);
        assert_eq!(
            asset_for(&one, "linux", "x86_64").unwrap().name,
            "newcraft-linux-amd64.AppImage"
        );
        assert_eq!(
            asset_for(&one, "linux", "aarch64").unwrap().name,
            "newcraft-linux-arm64.tar.gz"
        );
        let ambiguous = release(&[
            "newcraft-linux-amd64.AppImage",
            "newcraft-linux-x64.AppImage",
        ]);
        assert!(asset_for(&ambiguous, "linux", "x86_64").is_none());
        let misleading = release(&["newcraft-linux-amd64extra.AppImage"]);
        assert!(asset_for(&misleading, "linux", "x86_64").is_none());
    }

    #[test]
    fn explicit_package_selection_is_bounded_to_current_compatible_candidates() {
        let release = release(&[
            "app-linux-amd64.AppImage",
            "app-linux-x64.AppImage",
            "app-linux-arm64.AppImage",
            "app-linux-amd64.tar.gz",
        ]);
        assert!(
            matches!(resolve_asset(&release, "linux", "x86_64", None), AssetResolution::Ambiguous(v) if v.len() == 2)
        );
        assert_eq!(
            resolve_asset(&release, "linux", "x86_64", Some("app-linux-x64.AppImage"))
                .selected()
                .unwrap()
                .name,
            "app-linux-x64.AppImage"
        );
        assert!(matches!(
            resolve_asset(
                &release,
                "linux",
                "x86_64",
                Some("app-linux-arm64.AppImage")
            ),
            AssetResolution::Ambiguous(_)
        ));
        assert!(matches!(
            resolve_asset(&release, "linux", "x86_64", Some("stale-package.AppImage")),
            AssetResolution::Ambiguous(_)
        ));
        assert!(matches!(
            resolve_asset(&release, "linux", "x86_64", Some("app-linux-amd64.tar.gz")),
            AssetResolution::Ambiguous(_)
        ));
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

        let tgz_and_zip = self::tests::release(&[
            "app-linux-x86_64.tgz",
            "app-windows-x86_64.zip",
            "app-macos-arm64.zip",
        ]);
        assert_eq!(
            asset_for(&tgz_and_zip, "linux", "x86_64").unwrap().name,
            "app-linux-x86_64.tgz"
        );
        assert_eq!(
            asset_for(&tgz_and_zip, "windows", "x86_64").unwrap().name,
            "app-windows-x86_64.zip"
        );
        assert_eq!(
            asset_for(&tgz_and_zip, "macos", "aarch64").unwrap().name,
            "app-macos-arm64.zip"
        );
    }

    #[test]
    fn checksum_source_requires_valid_digest_or_matching_manifest() {
        let mut release = release(&[
            "app-linux-x86_64.tar.gz",
            "other-linux-x86_64-SHA256SUMS.txt",
            "app-linux-x86_64-SHA256SUMS.txt",
        ]);
        assert!(
            matches!(checksum_source(&release, &release.assets[0]), ChecksumSource::Manifest(a) if a.name == "app-linux-x86_64-SHA256SUMS.txt")
        );
        release.assets.pop();
        assert!(matches!(
            checksum_source(&release, &release.assets[0]),
            ChecksumSource::Missing
        ));
        release.assets[0].digest = Some(format!("sha256:{}", "a".repeat(64)));
        assert!(matches!(
            checksum_source(&release, &release.assets[0]),
            ChecksumSource::Digest(_)
        ));
        release.assets[0].digest = Some("sha256:not-a-hash".into());
        assert!(matches!(
            checksum_source(&release, &release.assets[0]),
            ChecksumSource::Missing
        ));
        release.assets.push(Asset {
            name: "SHA256SUMS.txt".into(),
            browser_download_url: String::new(),
            size: 1,
            digest: None,
        });
        assert!(
            matches!(checksum_source(&release, &release.assets[0]), ChecksumSource::Manifest(a) if a.name == "SHA256SUMS.txt")
        );
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

    #[test]
    fn release_cache_preserves_etag_across_channel_switches_and_handles_rate_limits() {
        let body = r#"[
            {"tag_name":"v2.0.0-rc.1","name":null,"body":null,"published_at":"2026-10-08T00:00:00Z","prerelease":true,"draft":false,"assets":[]},
            {"tag_name":"v1.0.0","name":null,"body":null,"published_at":"2026-10-07T00:00:00Z","prerelease":false,"draft":false,"assets":[]},
            {"tag_name":"v3.0.0","name":null,"body":null,"published_at":"2026-10-09T00:00:00Z","prerelease":false,"draft":true,"assets":[]}
        ]"#;
        let response = format!(
            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nETag: \"rel-v1\"\r\nConnection: close\r\n\r\n{}",
            body.len(),
            body
        );
        let client = Client::new();
        let (url, server) = serve_once(response);
        let base = url.trim_end_matches("/file");
        let stable = fetch_release_product(
            &client,
            base,
            Some("test-token"),
            "photocraft",
            Channel::Stable,
            None,
        )
        .unwrap();
        let req = server.join().unwrap().to_ascii_lowercase();
        assert!(req.contains("authorization: bearer test-token"));
        assert_eq!(
            stable.release.as_ref().map(|r| r.tag_name.as_str()),
            Some("v1.0.0")
        );
        assert_eq!(
            stable
                .versions_for(Channel::Stable)
                .map(|r| r.tag_name.as_str())
                .collect::<Vec<_>>(),
            vec!["v1.0.0"]
        );

        let (url, server) = serve_once("HTTP/1.1 304 Not Modified\r\nConnection: close\r\n\r\n");
        let base = url.trim_end_matches("/file");
        let preview = fetch_release_product(
            &client,
            base,
            None,
            "photocraft",
            Channel::Preview,
            Some(&stable),
        )
        .unwrap();
        assert!(
            server
                .join()
                .unwrap()
                .to_ascii_lowercase()
                .contains("if-none-match: \"rel-v1\"")
        );
        assert_eq!(preview.channel, Channel::Preview);
        assert_eq!(
            preview.release.as_ref().map(|r| r.tag_name.as_str()),
            Some("v2.0.0-rc.1")
        );
        assert_eq!(
            preview
                .versions_for(Channel::Preview)
                .map(|r| r.tag_name.as_str())
                .collect::<Vec<_>>(),
            vec!["v2.0.0-rc.1", "v1.0.0"]
        );

        let (url, server) = serve_once(
            "HTTP/1.1 429 Too Many Requests\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
        );
        let base = url.trim_end_matches("/file");
        let err = fetch_release_product(&client, base, None, "photocraft", Channel::Stable, None)
            .unwrap_err();
        assert!(matches!(err, crate::Error::RateLimited));
        server.join().unwrap();
    }
}
