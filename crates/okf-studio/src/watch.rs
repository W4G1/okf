//! A std-only polling file watcher.
//!
//! One `metadata()` sweep of the `*.md` files under the root every interval,
//! comparing `(mtime, size)`. Polling behaves identically on NFS and in
//! containers where inotify does not, and a bundle is at most a few thousand
//! small files, so a sweep costs single-digit milliseconds.
//!
//! The sweep uses the shared bundle walker, so a change inside an ignored
//! directory never triggers a reload: the watcher sees exactly the files the
//! snapshot loads, plus the ignore files that decide that set.

use crate::app::Msg;
use okf_core::bundle::LoadOptions;
use okf_core::walk::{WalkOptions, walk_entries_with_rule_files};
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

fn record(out: &mut FileState, path: &Path) {
    if let Ok(meta) = std::fs::metadata(path) {
        let mtime = meta.modified().unwrap_or(SystemTime::UNIX_EPOCH);
        out.insert(path.to_path_buf(), (mtime, meta.len()));
    }
}

fn sweep(root: &Path, load: &LoadOptions, out: &mut FileState) {
    // Both callbacks record into `out`, so it cannot be borrowed by each.
    let mut rule_files = Vec::new();
    // A walk error (root vanished mid-sweep, unreadable directory) leaves
    // `out` partial; the next sweep will differ and trigger a reload, which
    // is the right outcome.
    let _ = walk_entries_with_rule_files(
        root,
        &WalkOptions::new(&load.ignore),
        &mut |path, file_type| {
            if file_type.is_file() && path.extension().is_some_and(|e| e == "md") {
                record(out, path);
            }
        },
        &mut |path| rule_files.push(path.to_path_buf()),
    );
    for path in &rule_files {
        record(out, path);
    }
}

/// Spawns the watcher thread. It sends [`Msg::FilesChanged`] whenever the
/// `(mtime, size)` sweep differs from the previous one.
///
/// Files excluded by `load`'s ignore rules (or `.okfignore`) are not
/// watched; the ignore files themselves are.
#[must_use]
pub fn spawn(
    root: PathBuf,
    load: LoadOptions,
    interval: Duration,
    tx: Sender<Msg>,
) -> WatcherHandle {
    let stop = Arc::new(AtomicBool::new(false));
    let stop_flag = Arc::clone(&stop);
    std::thread::spawn(move || {
        let mut previous = FileState::new();
        sweep(&root, &load, &mut previous);
        while !stop_flag.load(Ordering::Relaxed) {
            std::thread::sleep(interval);
            if stop_flag.load(Ordering::Relaxed) {
                break;
            }
            let mut current = FileState::new();
            sweep(&root, &load, &mut current);
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
