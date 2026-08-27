//! A std-only polling file watcher.
//!
//! One `metadata()` sweep of the `*.md` files under the root every interval,
//! comparing `(mtime, size)`. Polling behaves identically on NFS and in
//! containers where inotify does not, and a bundle is at most a few thousand
//! small files, so a sweep costs single-digit milliseconds.

use crate::app::Msg;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Sender;
use std::time::{Duration, SystemTime};

/// Stops the watcher thread when dropped.
pub struct WatcherHandle {
    stop: Arc<AtomicBool>,
}

impl Drop for WatcherHandle {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

type FileState = HashMap<PathBuf, (SystemTime, u64)>;

fn sweep(root: &Path, out: &mut FileState) {
    let Ok(entries) = std::fs::read_dir(root) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        if file_type.is_dir() {
            sweep(&path, out);
        } else if file_type.is_file()
            && path.extension().is_some_and(|e| e == "md")
            && let Ok(meta) = entry.metadata()
        {
            let mtime = meta.modified().unwrap_or(SystemTime::UNIX_EPOCH);
            out.insert(path, (mtime, meta.len()));
        }
    }
}

/// Spawns the watcher thread. It sends [`Msg::FilesChanged`] whenever the
/// `(mtime, size)` sweep differs from the previous one.
#[must_use]
pub fn spawn(root: PathBuf, interval: Duration, tx: Sender<Msg>) -> WatcherHandle {
    let stop = Arc::new(AtomicBool::new(false));
    let stop_flag = Arc::clone(&stop);
    std::thread::spawn(move || {
        let mut previous = FileState::new();
        sweep(&root, &mut previous);
        while !stop_flag.load(Ordering::Relaxed) {
            std::thread::sleep(interval);
            if stop_flag.load(Ordering::Relaxed) {
                break;
            }
            let mut current = FileState::new();
            sweep(&root, &mut current);
            if current != previous {
                previous = current;
                if tx.send(Msg::FilesChanged).is_err() {
                    break;
                }
            }
        }
    });
    WatcherHandle { stop }
}
