use std::path::Path;
use std::time::Duration;

use crossbeam_channel::Sender;
use notify::{Config, Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher};

pub enum WatcherEvent {
    FileChanged,
}

pub struct FileWatcher {
    _watcher: RecommendedWatcher,
}

impl FileWatcher {
    pub fn new<P: AsRef<Path>>(
        path: P,
        sender: Sender<WatcherEvent>,
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

        Ok(Self { _watcher: watcher })
    }
}
