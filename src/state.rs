use std::{
    sync::{Arc, RwLock},
    thread,
};

use anyhow::Result;

use crate::{
    config::{AppConfig, FlockConfig, FlockProcessConfig},
    utils::{
        file_watcher::{FILE_WATCHER, FileWatcherStatus, WatcherEvent, ensure_watcher_initialized},
        process::{Process, ProcessRunningStatus, ProcessStatus, RestartDebounceHandler},
    },
};

// pub struct AppState {
//     pub active_ui: ActiveUIState,
//     flock_processes: Arc<Vec<FlockState>>,
// }

pub enum AppState {
    Main(MainUIState, GlobalUIState),
}

pub struct GlobalUIState {
    pub flocks: Vec<FlockState>,
}

impl AppState {
    pub fn new(config: AppConfig) -> Self {
        let flock_processes = config
            .flocks
            .into_iter()
            .map(|flock_cfg| FlockState::from(flock_cfg))
            .collect();

        Self::Main(
            MainUIState { active_flock: 0 },
            GlobalUIState {
                flocks: flock_processes,
            },
        )
    }

    pub fn next_item(&mut self) {
        match self {
            AppState::Main(state, global_state) => {
                state.next_flock(global_state.flocks.len());
            }
        }
    }
    pub fn previous_item(&mut self) {
        match self {
            AppState::Main(state, global_state) => {
                state.previous_flock(global_state.flocks.len());
            }
        }
    }

    pub fn select(&mut self) {
        match self {
            AppState::Main(state, global_state) => {
                state.launch_flock(&mut global_state.flocks);
            }
        }
    }
}

pub struct MainUIState {
    pub active_flock: usize,
}

impl MainUIState {
    fn next_flock(&mut self, no_of_flock: usize) {
        let mut next_flock_wrapped = self.active_flock + 1;
        if next_flock_wrapped == no_of_flock {
            next_flock_wrapped = 0
        }
        self.active_flock = next_flock_wrapped;
    }
    fn previous_flock(&mut self, no_of_flock: usize) {
        if self.active_flock == 0 {
            self.active_flock = no_of_flock - 1;
        } else {
            self.active_flock -= 1;
        };
    }
    fn launch_flock(&mut self, flocks: &mut Vec<FlockState>) {
        flocks
            .get_mut(self.active_flock)
            .expect("Flock should exists, but didn't")
            .processes
            .iter_mut()
            .for_each(|x| {
                x.launch().unwrap();
            });
    }
}

pub struct FlockState {
    pub display_name: String,
    pub processes: Vec<ProcessState>,
}

impl From<FlockConfig> for FlockState {
    fn from(config: FlockConfig) -> Self {
        Self {
            display_name: config.display_name,
            processes: config
                .processes
                .into_iter()
                .map(|process_cfg| ProcessState::new(process_cfg))
                .collect(),
        }
    }
}

pub struct ProcessState {
    pub process_config: Arc<FlockProcessConfig>,
    pub status: Arc<RwLock<ProcessStatus>>,
}

impl ProcessState {
    pub fn new(process_config: FlockProcessConfig) -> Self {
        Self {
            process_config: Arc::new(process_config),
            status: Arc::new(RwLock::new(ProcessStatus::Stopped)),
        }
    }

    pub fn launch(&mut self) -> Result<()> {
        fn is_launchable(status: &ProcessStatus) -> bool {
            status == &ProcessStatus::Stopped
        }

        let can_launch = {
            if let Ok(status) = self.status.read() {
                is_launchable(&*status)
            } else {
                false
            }
        };

        if can_launch {
            if let Ok(mut status) = self.status.write() {
                if is_launchable(&*status) {
                    // Initialize watcher lazily if this is a watchable process
                    if self.process_config.watch.is_enabled() {
                        self.enable_file_watching()
                    }

                    *status = ProcessStatus::Running(Process::new(
                        self.process_config.command.to_owned(),
                    )?);
                }
            }
        }

        Ok(())
    }
    fn enable_file_watching(&self) {
        ensure_watcher_initialized();
        let status = self.status.clone();
        let process_config = self.process_config.clone();

        // Subscribe to the file watcher bus
        let rx = if let Ok(watcher) = FILE_WATCHER.read() {
            match &*watcher {
                FileWatcherStatus::Enabled(watcher) => Some(watcher.subscribe()),
                _ => None,
            }
        } else {
            None
        };

        if let Some(mut receiver) = rx {
            thread::spawn(move || {
                loop {
                    if let Ok(WatcherEvent::FileChanged) = receiver.recv() {
                        if let Ok(mut s) = status.write() {
                            match &mut *s {
                                ProcessStatus::Stopped => break,
                                ProcessStatus::Running(process) => match &mut process.status {
                                    ProcessRunningStatus::Stable => {
                                        process.status = ProcessRunningStatus::Debouncing(
                                            RestartDebounceHandler::new(
                                                process_config.clone(),
                                                status.clone(),
                                            ),
                                        );
                                    }
                                    ProcessRunningStatus::Debouncing(timer) => {
                                        timer.reset();
                                    }
                                    ProcessRunningStatus::Restarting => {}
                                },
                            }
                        }
                    }
                }
            });
        }
    }
}
