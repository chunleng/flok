use std::mem::discriminant;
use std::path::Path;
use std::sync::{LazyLock, RwLock};
use std::time::Duration;

use anyhow::anyhow;
use crossbeam_channel::{Receiver, Sender, unbounded};
use notify::{Config, Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher};

pub static FILE_WATCHER: LazyLock<RwLock<FileWatcherStatus>> =
    LazyLock::new(|| RwLock::new(FileWatcherStatus::Disabled));

pub fn ensure_watcher_initialized() {
    let is_init = match FILE_WATCHER.read() {
        Ok(state) => match *state {
            FileWatcherStatus::Disabled => false,
            FileWatcherStatus::Enabled(_) => true,
        },
        _ => false,
    };

    if !is_init {
        if let Ok(mut status) = FILE_WATCHER.write() {
            if *status == FileWatcherStatus::Disabled {
                let (watcher_tx, watcher_rx) = unbounded();
                let cwd = std::env::current_dir().unwrap();
                let file_watcher = FileWatcher::new(&cwd, watcher_tx, watcher_rx)
                    .map_err(|e| anyhow!("Failed to initialize file watcher: {}", e))
                    .unwrap();
                *status = FileWatcherStatus::Enabled(file_watcher);
            }
        }
    }
}

pub enum WatcherEvent {
    FileChanged,
}

pub enum FileWatcherStatus {
    Disabled,
    Enabled(FileWatcher),
}

impl PartialEq for FileWatcherStatus {
    fn eq(&self, other: &Self) -> bool {
        discriminant(self) == discriminant(other)
    }
}

pub struct FileWatcher {
    pub watcher_rx: Receiver<WatcherEvent>,
    _watcher: RecommendedWatcher,
}

impl FileWatcher {
    pub fn new<P: AsRef<Path>>(
        path: P,
        sender: Sender<WatcherEvent>,
        receiver: Receiver<WatcherEvent>,
    ) -> Result<Self, notify::Error> {
        let mut watcher = RecommendedWatcher::new(
            move |res: Result<Event, notify::Error>| {
                if let Ok(event) = res {
                    // Only react to modify, create, and remove events
                    match event.kind {
                        EventKind::Modify(_) | EventKind::Create(_) | EventKind::Remove(_) => {
                            let _ = sender.send(WatcherEvent::FileChanged);
                        }
                        _ => {}
                    }
                }
            },
            Config::default().with_poll_interval(Duration::from_secs(1)),
        )?;

        watcher.watch(path.as_ref(), RecursiveMode::Recursive)?;

        Ok(Self {
            watcher_rx: receiver,
            _watcher: watcher,
        })
    }
}
