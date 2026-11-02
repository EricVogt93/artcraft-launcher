//! A detached monitor keeps output pipes drained even if the launcher exits.
use crate::{Result, fail, persistence::atomic_write};
use std::{
    collections::VecDeque,
    io::Read,
    path::Path,
    process::{Command, Stdio},
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
    time::{Duration, Instant},
};
const LIMIT: usize = 128 * 1024;

pub enum MonitorEvent {
    Started(u32),
    Exited(Option<i32>, bool),
}
pub fn capture(executable: &Path, log: &Path, mut event: impl FnMut(MonitorEvent)) -> Result<()> {
    atomic_write(log, b"")?;
    let mut command = Command::new(executable);
    command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if executable.extension().is_some_and(|ext| ext == "AppImage") {
        command.env("APPIMAGE_EXTRACT_AND_RUN", "1");
    }
    let mut attempts = 0;
    let mut child = loop {
        match command.spawn() {
            Ok(child) => break child,
            Err(error)
                if error.kind() == std::io::ErrorKind::ExecutableFileBusy && attempts < 5 =>
            {
                attempts += 1;
                std::thread::sleep(Duration::from_millis(50));
            }
            Err(error) => return Err(error.into()),
        }
    };
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| fail("Cannot capture app output."))?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| fail("Cannot capture app output."))?;
    let buffer = Arc::new(Mutex::new(VecDeque::<u8>::new()));
    let readers = Arc::new(AtomicUsize::new(2));
    fn drain(
        mut source: impl Read + Send + 'static,
        buffer: Arc<Mutex<VecDeque<u8>>>,
        readers: Arc<AtomicUsize>,
    ) {
        std::thread::spawn(move || {
            let mut chunk = [0u8; 8192];
            while let Ok(n) = source.read(&mut chunk) {
                if n == 0 {
                    break;
                }
                if let Ok(mut tail) = buffer.lock() {
                    for byte in &chunk[..n] {
                        if tail.len() == LIMIT {
                            tail.pop_front();
                        }
                        tail.push_back(*byte);
                    }
                } else {
                    break;
                }
            }
            readers.fetch_sub(1, Ordering::Release);
        });
    }
    drain(stdout, buffer.clone(), readers.clone());
    drain(stderr, buffer.clone(), readers.clone());
    event(MonitorEvent::Started(child.id()));
    let mut exited = false;
    let mut last_flush = Instant::now() - Duration::from_secs(1);
    while !exited || readers.load(Ordering::Acquire) != 0 {
        if !exited && let Some(status) = child.try_wait()? {
            exited = true;
            // Give readers a short opportunity to drain the just-exited process.
            for _ in 0..10 {
                if readers.load(Ordering::Acquire) == 0 {
                    break;
                }
                std::thread::sleep(Duration::from_millis(10));
            }
            if let Ok(tail) = buffer.lock() {
                let bytes: Vec<u8> = tail.iter().copied().collect();
                let _ = atomic_write(log, &bytes);
            }
            event(MonitorEvent::Exited(status.code(), status.success()));
        }
        if last_flush.elapsed() >= Duration::from_secs(1) {
            if let Ok(tail) = buffer.lock() {
                let bytes: Vec<u8> = tail.iter().copied().collect();
                let _ = atomic_write(log, &bytes);
            }
            last_flush = Instant::now();
        }
        if !exited || readers.load(Ordering::Acquire) != 0 {
            std::thread::sleep(Duration::from_millis(50));
        }
    }
    if let Ok(tail) = buffer.lock() {
        let bytes: Vec<u8> = tail.iter().copied().collect();
        atomic_write(log, &bytes)?;
    }
    Ok(())
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    #[test]
    fn output_is_bounded_and_reports_observed_failure() {
        let tmp = tempfile::tempdir().unwrap();
        let script = tmp.path().join("fail.sh");
        std::fs::write(
            &script,
            "#!/bin/sh\nhead -c 200000 /dev/zero\nprintf 'failed detail' >&2\nexit 7\n",
        )
        .unwrap();
        crate::installer::make_executable(&script).unwrap();
        let log = tmp.path().join("log");
        let mut events = Vec::new();
        capture(&script, &log, |event| events.push(event)).unwrap();
        assert!(matches!(
            events.as_slice(),
            [
                MonitorEvent::Started(_),
                MonitorEvent::Exited(Some(7), false)
            ]
        ));
        let bytes = std::fs::read(log).unwrap();
        assert_eq!(bytes.len(), LIMIT);
    }
    #[test]
    fn monitor_drains_output_from_child_after_parent_exits() {
        let tmp = tempfile::tempdir().unwrap();
        let script = tmp.path().join("forward.sh");
        std::fs::write(
            &script,
            "#!/bin/sh\n(sleep 0.2; printf 'later output') &\nexit 0\n",
        )
        .unwrap();
        crate::installer::make_executable(&script).unwrap();
        let log = tmp.path().join("log");
        let mut success = false;
        capture(&script, &log, |event| {
            if let MonitorEvent::Exited(_, ok) = event {
                success = ok;
            }
        })
        .unwrap();
        assert!(success);
        assert!(String::from_utf8_lossy(&std::fs::read(log).unwrap()).contains("later output"));
    }
}
