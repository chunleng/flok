mod components;
use std::{
    sync::{Arc, RwLock},
    time::{Duration, Instant},
};

use anyhow::anyhow;
use crossbeam_channel::{Receiver, Sender, unbounded};
use crossterm::event::{self, Event, KeyCode, KeyModifiers};
use nix::sys::signal::{Signal, kill};
use nix::unistd::Pid;
use ratatui::widgets::ListState;
use ratatui::{
    DefaultTerminal, Frame,
    buffer::Buffer,
    crossterm::event::poll,
    layout::{Constraint, Direction, Layout, Rect},
    prelude::*,
    widgets::Widget,
};

use crate::utils::file_watcher::{DebounceTimer, WatcherEvent};
use crate::utils::process::{ProcessState, ProcessStatus};
use crate::{
    config::AppConfig,
    error::{FlokProgramError, FlokProgramExecutionError, FlokProgramInitError},
};
use crate::{
    ui::components::lists::{SideListView, SplitListView},
    utils::file_watcher::{FILE_WATCHER, FileWatcherStatus},
};
use crate::{ui::components::pty::AutoFillPty, utils::process::ProcessRunningStatus};

pub fn run(config: AppConfig) -> Result<(), FlokProgramError> {
    let mut terminal = ratatui::init();
    let app_result = App::new(config)
        .map_err(|e| FlokProgramError::Init(FlokProgramInitError::Unknown(e.into())))?
        .run(&mut terminal);
    ratatui::restore();

    app_result
}

struct App {
    exit: bool,
    config: AppConfig,
    flock_state: ListState,
    flock_processes: Vec<Vec<ProcessState>>,
    shutdown_complete_rx: Receiver<(usize, usize)>,
    shutdown_complete_tx: Sender<(usize, usize)>,
}

impl App {
    fn new(config: AppConfig) -> Result<Self, anyhow::Error> {
        let mut flock_state = ListState::default();
        flock_state.select(Some(0));
        let flock_processes: Vec<Vec<_>> = config
            .flocks
            .iter()
            .map(|flock_cfg| {
                flock_cfg
                    .processes
                    .iter()
                    .map(|process_cfg| ProcessState::new(process_cfg.clone()))
                    .collect()
            })
            .collect();

        let (shutdown_complete_tx, shutdown_complete_rx) = unbounded();

        Ok(Self {
            exit: false,
            config,
            flock_state,
            flock_processes,
            shutdown_complete_rx,
            shutdown_complete_tx,
        })
    }
    fn run(&mut self, terminal: &mut DefaultTerminal) -> Result<(), FlokProgramError> {
        while !self.exit {
            terminal
                .draw(|frame| self.draw(frame))
                .map_err(|e| FlokProgramError::Init(e.into()))?;
            self.handle_event()
                .map_err(|e| FlokProgramError::Execution(e.into()))?;
        }
        Ok(())
    }

    fn draw(&mut self, frame: &mut Frame) {
        frame.render_widget(self, frame.area());
    }

    fn graceful_shutdown_async(
        child: Arc<RwLock<Box<dyn portable_pty::Child + Send + Sync>>>,
        timeout: Duration,
        completion_sender: Sender<(usize, usize)>,
        flock_idx: usize,
        process_idx: usize,
    ) {
        std::thread::spawn(move || {
            // Get the process ID
            let pid = {
                let child_lock = child.read().unwrap();
                match child_lock.process_id() {
                    Some(pid) => pid,
                    None => {
                        // No PID, notify completion and exit
                        let _ = completion_sender.send((flock_idx, process_idx));
                        return;
                    }
                }
            };
            let nix_pid = Pid::from_raw(pid as i32);

            // Send SIGTERM
            let _ = kill(nix_pid, Signal::SIGTERM);

            // Wait for process to exit with timeout
            let start = Instant::now();
            loop {
                let exit_status = {
                    let mut child_lock = child.write().unwrap();
                    child_lock.try_wait()
                };

                match exit_status {
                    Ok(Some(_)) => {
                        // Process exited, notify completion
                        let _ = completion_sender.send((flock_idx, process_idx));
                        return;
                    }
                    Ok(None) => {
                        // Still running, check timeout
                        if start.elapsed() >= timeout {
                            // Timeout exceeded, send SIGKILL
                            let _ = kill(nix_pid, Signal::SIGKILL);
                            // Wait a bit for SIGKILL to take effect
                            std::thread::sleep(Duration::from_millis(100));
                            let _ = child.write().unwrap().try_wait();
                            // Notify completion after SIGKILL
                            let _ = completion_sender.send((flock_idx, process_idx));
                            return;
                        }
                        std::thread::sleep(Duration::from_millis(50));
                    }
                    Err(_) => {
                        // Error checking, assume exited, notify completion
                        let _ = completion_sender.send((flock_idx, process_idx));
                        return;
                    }
                }
            }
        });
    }

    fn restart_process(
        &mut self,
        flock_idx: usize,
        process_idx: usize,
    ) -> Result<(), FlokProgramExecutionError> {
        // Set state to Restarting and clone the child Arc for background shutdown
        if let Some(processes) = self.flock_processes.get_mut(flock_idx) {
            if let Some(state) = processes.get_mut(process_idx) {
                if let ProcessStatus::Running(process) = &mut state.status {
                    if let ProcessRunningStatus::Debouncing(_) = process.status {
                        process.status = ProcessRunningStatus::Restarting;

                        // Spawn graceful shutdown in background thread (non-blocking)
                        // When complete, it will send a message to shutdown_complete_rx
                        Self::graceful_shutdown_async(
                            process.child.clone(),
                            Duration::from_secs(5),
                            self.shutdown_complete_tx.clone(),
                            flock_idx,
                            process_idx,
                        );
                    }
                }
            }
        }

        // Don't launch new process here - wait for shutdown completion
        Ok(())
    }

    fn process_debounce_timers(&mut self) -> Result<(), FlokProgramExecutionError> {
        let mut to_restart = Vec::new();

        for (flock_idx, processes) in self.flock_processes.iter().enumerate() {
            for (process_idx, process_state) in processes.iter().enumerate() {
                if let ProcessStatus::Running(process) = &process_state.status {
                    if let ProcessRunningStatus::Debouncing(timer) = &process.status {
                        if timer.is_expired() {
                            to_restart.push((flock_idx, process_idx));
                        }
                    }
                }
            }
        }

        for (flock_idx, process_idx) in to_restart {
            self.restart_process(flock_idx, process_idx)?;
        }

        Ok(())
    }

    fn handle_file_change(&mut self) -> Result<(), FlokProgramExecutionError> {
        if let Some(flock_idx) = self.flock_state.selected() {
            let flock = self
                .config
                .flocks
                .get(flock_idx)
                .ok_or(anyhow!("Selected flock does not exist"))?;

            for (process_idx, process_config) in flock.processes.iter().enumerate() {
                if !process_config.watch.is_enabled() {
                    continue;
                }

                if let Some(processes) = self.flock_processes.get_mut(flock_idx) {
                    if let Some(state) = processes.get_mut(process_idx) {
                        let debounce_duration = process_config.watch.debounce_duration();

                        match &mut state.status {
                            ProcessStatus::Stopped => {}
                            ProcessStatus::Running(process) => match &mut process.status {
                                ProcessRunningStatus::Stable => {
                                    process.status = ProcessRunningStatus::Debouncing(
                                        DebounceTimer::new(debounce_duration),
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
        }
        Ok(())
    }

    fn handle_event(&mut self) -> Result<(), FlokProgramExecutionError> {
        // Check for file watcher events only if watcher is initialized
        if let Ok(watcher) = FILE_WATCHER.read() {
            match *watcher {
                FileWatcherStatus::Enabled(ref watcher) => {
                    if let Ok(WatcherEvent::FileChanged) = watcher.watcher_rx.try_recv() {
                        self.handle_file_change()?;
                    }
                }
                _ => {}
            }
        }

        // Process expired debounce timers
        self.process_debounce_timers()?;

        // Check for shutdown completion events
        if let Ok((flock_idx, process_idx)) = self.shutdown_complete_rx.try_recv() {
            // Remove the old process (which is now terminated)
            if let Some(processes) = self.flock_processes.get_mut(flock_idx) {
                if let Some(process) = processes.get_mut(process_idx) {
                    process.launch().unwrap();
                }
            }
        }

        if poll(Duration::from_millis(100))? {
            match event::read()? {
                Event::Key(k) => match (k.modifiers, k.code) {
                    (KeyModifiers::CONTROL, KeyCode::Char('c'))
                    | (KeyModifiers::NONE, KeyCode::Char('q')) => {
                        self.exit = true;
                    }
                    (KeyModifiers::NONE, KeyCode::Char('j') | KeyCode::Down) => {
                        self.flock_state.select_next();
                    }
                    (KeyModifiers::NONE, KeyCode::Char('k') | KeyCode::Up) => {
                        self.flock_state.select_previous();
                    }
                    (KeyModifiers::NONE, KeyCode::Enter) => {
                        if let Some(flock_idx) = self.flock_state.selected() {
                            self.flock_processes
                                .get_mut(flock_idx)
                                .expect("Flock should exists, but didn't")
                                .iter_mut()
                                .for_each(|x| {
                                    x.launch().unwrap();
                                });
                        }
                    }
                    _ => {}
                },
                _ => {}
            }
        }
        Ok(())
    }
}

impl Widget for &mut App {
    fn render(self, area: Rect, buf: &mut Buffer) {
        let [sidebar_area, main_area] = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Percentage(20), Constraint::Fill(1)])
            .areas(area);
        SideListView::new(
            "Flocks".to_string(),
            self.config
                .flocks
                .iter()
                .map(|f| f.display_name.to_owned())
                .collect(),
        )
        .render(sidebar_area, buf, &mut self.flock_state);

        // Display processes for the selected flock
        if let Some(selected_flock_idx) = self.flock_state.selected() {
            let mut widgets = Vec::new();
            self.flock_processes
                .get(selected_flock_idx)
                .unwrap()
                .iter()
                .for_each(|state| {
                    match state.status {
                        ProcessStatus::Running(ref process) => {
                            // Build title with state indicator
                            let state_indicator = match &process.status {
                                ProcessRunningStatus::Restarting => " [Restarting...]",
                                _ => "",
                            };
                            let title =
                                format!("{}{}", state.process_config.display_name, state_indicator);

                            widgets.push(AutoFillPty::new(
                                process.pty_master.clone(),
                                process.parser.clone(),
                                title,
                            ));
                        }
                        _ => {}
                    }
                });

            SplitListView::new(widgets).render(main_area, buf)
        }
    }
}
