//! Local, signed self-update feed. A separate helper switches a version pointer only
//! after the launcher releases its library lock, and waits for a first-frame health ack.
use crate::{
    Result, fail,
    installer::{extract_tar, extract_zip, make_executable, safe_native_relative, safe_relative},
    model::{Channel, is_newer},
    persistence::atomic_json,
};
use ed25519_dalek::{Signature, VerifyingKey};
use fs2::FileExt;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    fs,
    io::Read,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::atomic::AtomicBool,
    time::{Duration, Instant},
};

pub const MANIFEST_SCHEMA: u32 = 1;
pub fn build_version() -> &'static str {
    option_env!("CRAFTLAUNCHER_BUILD_VERSION").unwrap_or(env!("CARGO_PKG_VERSION"))
}
pub fn target_id() -> String {
    format!("{}-{}", std::env::consts::OS, std::env::consts::ARCH)
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpdateManifest {
    pub schema: u32,
    pub version: String,
    pub channel: Channel,
    pub target: String,
    pub archive: String,
    pub sha256: String,
    pub bytes: u64,
    pub executable: PathBuf,
    pub helper: PathBuf,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PreparedUpdate {
    pub version: String,
    pub directory: PathBuf,
    pub executable: PathBuf,
    pub helper: PathBuf,
    pub executable_sha256: String,
    pub helper_sha256: String,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BootPointer {
    pub version: String,
    pub executable: PathBuf,
}
#[derive(Debug, Serialize, Deserialize)]
pub struct UpdateJob {
    pub prepared: PreparedUpdate,
    pub previous: BootPointer,
    pub public_key: String,
    pub timeout_seconds: u64,
}
#[derive(Debug, Serialize, Deserialize)]
pub struct UpdateReport {
    pub version: String,
    pub success: bool,
    pub restored: Option<String>,
}

pub fn hash_file(path: &Path) -> Result<String> {
    let mut file = fs::File::open(path)?;
    let mut hash = Sha256::new();
    let mut bytes = [0u8; 65536];
    loop {
        let n = file.read(&mut bytes)?;
        if n == 0 {
            break;
        }
        hash.update(&bytes[..n]);
    }
    Ok(format!("{:x}", hash.finalize()))
}
pub fn verify_manifest(
    bytes: &[u8],
    signature_hex: &str,
    public_key_hex: &str,
) -> Result<UpdateManifest> {
    let key_bytes: [u8; 32] = hex::decode(public_key_hex.trim())
        .map_err(|_| fail("Invalid update public key."))?
        .try_into()
        .map_err(|_| fail("Update public keys must contain 32 bytes."))?;
    let key =
        VerifyingKey::from_bytes(&key_bytes).map_err(|_| fail("Invalid update public key."))?;
    let signature = Signature::from_slice(
        &hex::decode(signature_hex.trim()).map_err(|_| fail("Invalid update signature."))?,
    )
    .map_err(|_| fail("Invalid update signature."))?;
    key.verify_strict(bytes, &signature)
        .map_err(|_| fail("The launcher update signature is invalid."))?;
    let manifest: UpdateManifest = serde_json::from_slice(bytes)?;
    if manifest.schema != MANIFEST_SCHEMA
        || crate::model::version_number(&manifest.version).is_none()
        || manifest.target != target_id()
    {
        return Err(fail(
            "The update manifest does not match this launcher or platform.",
        ));
    }
    if manifest.bytes == 0
        || manifest.bytes > 2 * 1024 * 1024 * 1024
        || manifest.sha256.len() != 64
        || !manifest.sha256.bytes().all(|b| b.is_ascii_hexdigit())
    {
        return Err(fail("Invalid update archive metadata."));
    }
    if !safe_relative(Path::new(&manifest.archive))
        || Path::new(&manifest.archive).components().count() != 1
        || !safe_relative(&manifest.executable)
        || !safe_relative(&manifest.helper)
    {
        return Err(fail("Unsafe paths in the update manifest."));
    }
    Ok(manifest)
}
fn limited_text(path: &Path) -> Result<Vec<u8>> {
    let file = fs::File::open(path)?;
    if file.metadata()?.len() > 65536 {
        return Err(fail("The update manifest is too large."));
    }
    let mut bytes = Vec::new();
    file.take(65537).read_to_end(&mut bytes)?;
    if bytes.len() > 65536 {
        return Err(fail("The update manifest is too large."));
    }
    Ok(bytes)
}
fn feed_files(feed: &Path, channel: Channel) -> (PathBuf, PathBuf) {
    let channel = if channel == Channel::Stable {
        "stable"
    } else {
        "preview"
    };
    let base = format!("manifest-{}-{channel}.json", target_id());
    (feed.join(&base), feed.join(format!("{base}.sig")))
}
pub fn prepare(
    root: &Path,
    feed: &Path,
    public_key: &str,
    channel: Channel,
    current_version: &str,
    cancel: &AtomicBool,
) -> Result<Option<PreparedUpdate>> {
    let feed = fs::canonicalize(feed)?;
    let (manifest_file, signature_file) = feed_files(&feed, channel);
    let bytes = limited_text(&manifest_file)?;
    let signature = String::from_utf8(limited_text(&signature_file)?)
        .map_err(|_| fail("Invalid signature file."))?;
    let manifest = verify_manifest(&bytes, &signature, public_key)?;
    if manifest.channel != channel {
        return Err(fail(
            "The update feed channel does not match the selected channel.",
        ));
    }
    if !is_newer(&manifest.version, current_version) {
        return Ok(None);
    }
    let source = fs::canonicalize(feed.join(&manifest.archive))?;
    if !source.starts_with(&feed) || source.metadata()?.len() != manifest.bytes {
        return Err(fail(
            "The update archive is missing or has an invalid size.",
        ));
    }
    if hash_file(&source)? != manifest.sha256.to_ascii_lowercase() {
        return Err(fail("Launcher archive checksum mismatch."));
    }
    let parent = root.join("launcher/versions");
    fs::create_dir_all(&parent)?;
    let stage = tempfile::Builder::new()
        .prefix(".staging-")
        .tempdir_in(&parent)?;
    let archive = stage.path().join("archive");
    fs::copy(&source, &archive)?;
    // Verify the actual copy, closing a source-file substitution race.
    if hash_file(&archive)? != manifest.sha256.to_ascii_lowercase() {
        return Err(fail("Launcher archive changed while preparing the update."));
    }
    let payload = stage.path().join("payload");
    fs::create_dir(&payload)?;
    if manifest.archive.ends_with(".tar.gz") {
        extract_tar(&archive, &payload, cancel, cfg!(target_os = "macos"))?;
    } else if manifest.archive.ends_with(".zip") {
        extract_zip(&archive, &payload, cancel, cfg!(target_os = "macos"))?;
    } else {
        return Err(fail("Unsupported launcher update archive format."));
    }
    let executable = payload.join(&manifest.executable);
    let helper = payload.join(&manifest.helper);
    if !executable.is_file() || !helper.is_file() {
        return Err(fail(
            "The launcher update has no executable or update helper.",
        ));
    }
    make_executable(&executable)?;
    make_executable(&helper)?;
    let executable_sha256 = hash_file(&executable)?;
    let helper_sha256 = hash_file(&helper)?;
    fs::write(stage.path().join("manifest.json"), &bytes)?;
    fs::write(stage.path().join("manifest.json.sig"), signature)?;
    let token = uuid::Uuid::new_v4().to_string();
    let destination = parent.join(&token);
    fs::rename(stage.path(), &destination)?;
    let directory = PathBuf::from("launcher/versions").join(token);
    Ok(Some(PreparedUpdate {
        version: manifest.version,
        executable: directory.join("payload").join(manifest.executable),
        helper: directory.join("payload").join(manifest.helper),
        directory,
        executable_sha256,
        helper_sha256,
    }))
}
fn within(root: &Path, relative: &Path) -> Result<PathBuf> {
    if !safe_native_relative(relative) || !relative.starts_with("launcher/versions") {
        return Err(fail("Invalid launcher update path."));
    }
    let path = fs::canonicalize(root.join(relative))?;
    if !path.starts_with(fs::canonicalize(root.join("launcher/versions"))?) {
        return Err(fail("The launcher update escapes its version directory."));
    }
    Ok(path)
}
pub fn start_helper(
    root: &Path,
    prepared: &PreparedUpdate,
    previous_executable: &Path,
    public_key: &str,
    timeout_seconds: u64,
) -> Result<()> {
    verify_prepared(root, prepared, public_key)?;
    let helper = within(root, &prepared.helper)?;
    if hash_file(&helper)? != prepared.helper_sha256 {
        return Err(fail("The update helper checksum has changed."));
    }
    let previous = BootPointer {
        version: build_version().into(),
        executable: fs::canonicalize(previous_executable)?,
    };
    let job = UpdateJob {
        prepared: prepared.clone(),
        previous,
        public_key: public_key.into(),
        timeout_seconds: timeout_seconds.clamp(1, 120),
    };
    let job_file = root.join("launcher/update-job.json");
    atomic_json(&job_file, &job)?;
    let result = Command::new(helper)
        .arg("--data-dir")
        .arg(root)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn();
    if let Err(error) = result {
        let _ = fs::remove_file(job_file);
        return Err(error.into());
    }
    Ok(())
}

/// Bind extracted executables to the signed archive, even if local state was modified.
pub fn verify_prepared(
    root: &Path,
    prepared: &PreparedUpdate,
    public_key: &str,
) -> Result<UpdateManifest> {
    let directory = within(root, &prepared.directory)?;
    let manifest = verify_manifest(
        &limited_text(&directory.join("manifest.json"))?,
        &String::from_utf8(limited_text(&directory.join("manifest.json.sig"))?)
            .map_err(|_| fail("Invalid signature."))?,
        public_key,
    )?;
    if manifest.version != prepared.version
        || prepared.executable
            != prepared
                .directory
                .join("payload")
                .join(&manifest.executable)
        || prepared.helper != prepared.directory.join("payload").join(&manifest.helper)
        || hash_file(&directory.join("archive"))? != manifest.sha256.to_ascii_lowercase()
    {
        return Err(fail(
            "The prepared launcher update failed its integrity check.",
        ));
    }
    let verify = tempfile::tempdir_in(&directory)?;
    let cancel = AtomicBool::new(false);
    if manifest.archive.ends_with(".tar.gz") {
        extract_tar(
            &directory.join("archive"),
            verify.path(),
            &cancel,
            cfg!(target_os = "macos"),
        )?;
    } else if manifest.archive.ends_with(".zip") {
        extract_zip(
            &directory.join("archive"),
            verify.path(),
            &cancel,
            cfg!(target_os = "macos"),
        )?;
    } else {
        return Err(fail("Unsupported update archive."));
    }
    for (relative, signed) in [
        (&prepared.executable, &manifest.executable),
        (&prepared.helper, &manifest.helper),
    ] {
        if hash_file(&within(root, relative)?)? != hash_file(&verify.path().join(signed))? {
            return Err(fail(
                "An extracted update executable differs from the signed archive.",
            ));
        }
    }
    Ok(manifest)
}

/// Called only by the separate update helper process.
pub fn apply_job(root: &Path) -> Result<bool> {
    let result = apply_verified_job(root);
    if let Err(error) = &result {
        let job_path = root.join("launcher/update-job.json");
        if job_path.is_file() {
            let job: UpdateJob = serde_json::from_slice(&limited_text(&job_path)?)?;
            atomic_json(&root.join("launcher/current.json"), &job.previous)?;
            atomic_json(
                &root.join("launcher/last-update.json"),
                &UpdateReport {
                    version: job.prepared.version,
                    success: false,
                    restored: Some(job.previous.version),
                },
            )?;
            fs::remove_file(job_path)?;
            Command::new(job.previous.executable)
                .arg("--data-dir")
                .arg(root)
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
                .map_err(|recovery| {
                    fail(format!(
                        "{error}; could not restart the previous launcher: {recovery}"
                    ))
                })?;
        }
    }
    result
}
fn apply_verified_job(root: &Path) -> Result<bool> {
    let root = fs::canonicalize(root)?;
    let job: UpdateJob =
        serde_json::from_slice(&limited_text(&root.join("launcher/update-job.json"))?)?;
    let executable = within(&root, &job.prepared.executable)?;
    verify_prepared(&root, &job.prepared, &job.public_key)?;
    let lock = fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(root.join(".lock"))?;
    let deadline = Instant::now() + Duration::from_secs(60);
    loop {
        if lock.try_lock_exclusive().is_ok() {
            break;
        }
        if Instant::now() > deadline {
            return Err(fail("The previous launcher did not exit."));
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    let pointer = root.join("launcher/current.json");
    let next = BootPointer {
        version: job.prepared.version.clone(),
        executable: executable.clone(),
    };
    atomic_json(&pointer, &next)?;
    let health = root
        .join("launcher")
        .join(format!("health-{}", uuid::Uuid::new_v4()));
    // Release before starting the new UI; it acquires the same library lock.
    FileExt::unlock(&lock)?;
    let mut child = Command::new(&executable)
        .arg("--data-dir")
        .arg(&root)
        .arg("--update-health")
        .arg(&health)
        .arg("--expected-version")
        .arg(&next.version)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn();
    let deadline = Instant::now() + Duration::from_secs(job.timeout_seconds.clamp(1, 120));
    let healthy = loop {
        if fs::read_to_string(&health).is_ok_and(|s| s.trim() == next.version) {
            break true;
        }
        match &mut child {
            Ok(child) => {
                if child.try_wait()?.is_some() {
                    break false;
                }
            }
            Err(_) => break false,
        }
        if Instant::now() >= deadline {
            break false;
        }
        std::thread::sleep(Duration::from_millis(100));
    };
    let _ = fs::remove_file(&health);
    if healthy {
        // State mutation is performed by the new launcher after first-frame acknowledgement.
        atomic_json(
            &root.join("launcher/last-update.json"),
            &UpdateReport {
                version: next.version,
                success: true,
                restored: None,
            },
        )?;
    } else {
        if let Ok(child) = &mut child {
            let _ = child.kill();
            let _ = child.wait();
        }
        atomic_json(&pointer, &job.previous)?;
        atomic_json(
            &root.join("launcher/last-update.json"),
            &UpdateReport {
                version: next.version,
                success: false,
                restored: Some(job.previous.version),
            },
        )?;
        let _ = fs::remove_file(root.join("launcher/update-job.json"));
        Command::new(&job.previous.executable)
            .arg("--data-dir")
            .arg(&root)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()?;
    }
    let _ = fs::remove_file(root.join("launcher/update-job.json"));
    Ok(healthy)
}

pub fn current_executable(root: &Path) -> Result<Option<PathBuf>> {
    // A helper that crashed before acknowledgement must not make an unproven
    // executable the permanent bootstrap target. Normal start retries from the old UI.
    let job_file = root.join("launcher/update-job.json");
    if job_file.is_file() {
        let job: UpdateJob = serde_json::from_slice(&limited_text(&job_file)?)?;
        if job.previous.executable.is_absolute() && job.previous.executable.is_file() {
            return Ok(Some(job.previous.executable));
        }
    }
    let path = root.join("launcher/current.json");
    if !path.exists() {
        return Ok(None);
    }
    let pointer: BootPointer = serde_json::from_slice(&limited_text(&path)?)?;
    if !pointer.executable.is_absolute() || !pointer.executable.is_file() {
        return Err(fail("The active launcher version is missing."));
    }
    Ok(Some(pointer.executable))
}
