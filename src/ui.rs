use std::io::Write;
use std::{
    collections::HashMap,
    io::Read,
    sync::{Arc, RwLock},
    time::{Duration, Instant},
};

use anyhow::anyhow;
use crossbeam_channel::{Receiver, Sender, unbounded};
use crossterm::event::{self, Event, KeyCode, KeyModifiers};
use nix::sys::signal::{Signal, kill};
use nix::unistd::Pid;
use portable_pty::{CommandBuilder, PtySize, native_pty_system};
use ratatui::{
    DefaultTerminal, Frame,
    buffer::Buffer,
    crossterm::event::poll,
    layout::{Constraint, Direction, Layout, Rect},
    prelude::*,
    style::{Style, Stylize},
    text::Line,
    widgets::{Block, Borders, Paragraph, Widget},
};
use tempfile::NamedTempFile;
use tui_widget_list::{ListBuilder, ListState, ListView};

use crate::{
    Config, Flock,
    error::{FlokProgramError, FlokProgramExecutionError, FlokProgramInitError},
    watcher::{FileWatcher, WatcherEvent},
};

pub fn run(config: Config) -> Result<(), FlokProgramError> {
    let mut terminal = ratatui::init();
    let app_result = App::new(config)
        .map_err(|e| FlokProgramError::Init(FlokProgramInitError::Unknown(e.into())))?
        .run(&mut terminal);
    ratatui::restore();

    app_result
}

#[derive(Debug, Clone, PartialEq)]
struct DebounceTimer {
    started_at: Instant,
    duration: Duration,
}

impl DebounceTimer {
    fn new(duration: Duration) -> Self {
        Self {
            started_at: Instant::now(),
            duration,
        }
    }

    fn reset(&mut self) {
        self.started_at = Instant::now();
    }

    fn is_expired(&self) -> bool {
        self.started_at.elapsed() >= self.duration
    }
}

#[derive(Debug, Clone, PartialEq)]
enum ProcessState {
    Running,
    Restarting,
    RestartDebouncing(DebounceTimer),
}

struct Process {
    child: Arc<RwLock<Box<dyn portable_pty::Child + Send + Sync>>>,
    pty_master: Box<dyn portable_pty::MasterPty + Send>,
    parser: Arc<RwLock<vt100::Parser>>,
    state: ProcessState,
}

struct App {
    exit: bool,
    config: Config,
    flock_state: ListState,
    flock_processes: HashMap<usize, HashMap<usize, Process>>,
    watcher_rx: Option<Receiver<WatcherEvent>>,
    _file_watcher: Option<FileWatcher>,
    shutdown_complete_rx: Receiver<(usize, usize)>,
    shutdown_complete_tx: Sender<(usize, usize)>,
}

impl App {
    fn new(config: Config) -> Result<Self, anyhow::Error> {
        let mut flock_state = ListState::default();
        flock_state.select(Some(0));

        let (shutdown_complete_tx, shutdown_complete_rx) = unbounded();

        Ok(Self {
            exit: false,
            config,
            flock_state,
            flock_processes: HashMap::new(),
            watcher_rx: None,
            _file_watcher: None,
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

    fn ensure_watcher_initialized(&mut self) -> Result<(), FlokProgramExecutionError> {
        if self._file_watcher.is_none() {
            let (watcher_tx, watcher_rx) = unbounded();
            let cwd = std::env::current_dir()?;
            let file_watcher = FileWatcher::new(&cwd, watcher_tx)
                .map_err(|e| anyhow!("Failed to initialize file watcher: {}", e))?;
            self.watcher_rx = Some(watcher_rx);
            self._file_watcher = Some(file_watcher);
        }
        Ok(())
    }

    fn launch_process(
        &mut self,
        flock_idx: usize,
        process_idx: usize,
    ) -> Result<(), FlokProgramExecutionError> {
        // Get the process config and check watch flag before any borrows
        let (command, watch_enabled) = {
            let flock = self
                .config
                .flocks
                .get(flock_idx)
                .ok_or(anyhow!("Flock does not exist"))?;
            let flock_process = flock
                .processes
                .get(process_idx)
                .ok_or(anyhow!("Process does not exist"))?;
            (
                flock_process.command.clone(),
                flock_process.watch.is_enabled(),
            )
        };

        // Initialize watcher lazily if this is a watchable process
        if watch_enabled {
            self.ensure_watcher_initialized()?;
        }

        let processes = self
            .flock_processes
            .entry(flock_idx)
            .or_insert_with(HashMap::new);

        // Launch the process using PTY for proper interactive support
        let pty_system = native_pty_system();
        let pair = pty_system
            .openpty(PtySize {
                rows: 24,
                cols: 80,
                pixel_width: 0,
                pixel_height: 0,
            })
            .map_err(|e| anyhow!("Failed to open PTY: {}", e))?;

        let mut script = NamedTempFile::new()?;
        let script_path = script.path().display().to_string();
        writeln!(script, "{}", command)?;
        let _ = script.persist(script_path.clone());

        // Use the login shell from SHELL environment variable, fallback to sh
        let shell = std::env::var("SHELL").unwrap_or("sh".to_string());
        let mut cmd = CommandBuilder::new(shell);
        cmd.arg(script_path);
        cmd.cwd(std::env::current_dir()?);

        let child = pair
            .slave
            .spawn_command(cmd)
            .map_err(|e| anyhow!("Failed to spawn command: {}", e))?;

        let mut reader = pair
            .master
            .try_clone_reader()
            .map_err(|e| anyhow!("Failed to clone PTY reader: {}", e))?;

        // Create a VT100 parser to handle terminal escape sequences
        let parser = Arc::new(RwLock::new(vt100::Parser::new(24, 80, 0)));
        let parser_clone = parser.clone();

        std::thread::spawn(move || {
            loop {
                let mut buffer = [0; 8192];
                let bytes_read = match reader.read(&mut buffer) {
                    Ok(n) => n,
                    Err(_) => break,
                };
                if bytes_read == 0 {
                    break;
                }
                // Feed the output to the VT100 parser
                parser_clone.write().unwrap().process(&buffer[..bytes_read]);
            }
        });

        processes.insert(
            process_idx,
            Process {
                child: Arc::new(RwLock::new(child)),
                pty_master: pair.master,
                parser,
                state: ProcessState::Running,
            },
        );

        Ok(())
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
        // Check if process exists and get its current state
        let process_state = self
            .flock_processes
            .get(&flock_idx)
            .and_then(|p| p.get(&process_idx))
            .map(|p| p.state.clone());

        match process_state {
            None => return Ok(()),                           // Process doesn't exist
            Some(ProcessState::Restarting) => return Ok(()), // Already restarting, skip
            Some(ProcessState::Running) | Some(ProcessState::RestartDebouncing(_)) => {}
        }

        // Set state to Restarting and clone the child Arc for background shutdown
        if let Some(processes) = self.flock_processes.get_mut(&flock_idx) {
            if let Some(process) = processes.get_mut(&process_idx) {
                process.state = ProcessState::Restarting;

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

        // Don't launch new process here - wait for shutdown completion
        Ok(())
    }

    fn process_debounce_timers(&mut self) -> Result<(), FlokProgramExecutionError> {
        let mut to_restart = Vec::new();

        for (flock_idx, processes) in &self.flock_processes {
            for (process_idx, process) in processes.iter() {
                if let ProcessState::RestartDebouncing(timer) = &process.state {
                    if timer.is_expired() {
                        to_restart.push((*flock_idx, *process_idx));
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
        if let Some(flock_idx) = self.flock_state.selected {
            let flock = self
                .config
                .flocks
                .get(flock_idx)
                .ok_or(anyhow!("Selected flock does not exist"))?;

            for (process_idx, process_config) in flock.processes.iter().enumerate() {
                if !process_config.watch.is_enabled() {
                    continue;
                }

                if let Some(processes) = self.flock_processes.get_mut(&flock_idx) {
                    if let Some(process) = processes.get_mut(&process_idx) {
                        let debounce_duration = process_config.watch.debounce_duration();

                        match &mut process.state {
                            ProcessState::Running => {
                                process.state = ProcessState::RestartDebouncing(
                                    DebounceTimer::new(debounce_duration),
                                );
                            }
                            ProcessState::RestartDebouncing(timer) => {
                                timer.reset();
                            }
                            ProcessState::Restarting => {}
                        }
                    }
                }
            }
        }
        Ok(())
    }

    fn handle_event(&mut self) -> Result<(), FlokProgramExecutionError> {
        // Check for file watcher events only if watcher is initialized
        if let Some(rx) = &self.watcher_rx {
            if let Ok(WatcherEvent::FileChanged) = rx.try_recv() {
                self.handle_file_change()?;
            }
        }

        // Process expired debounce timers
        self.process_debounce_timers()?;

        // Check for shutdown completion events
        if let Ok((flock_idx, process_idx)) = self.shutdown_complete_rx.try_recv() {
            // Remove the old process (which is now terminated)
            if let Some(processes) = self.flock_processes.get_mut(&flock_idx) {
                processes.remove(&process_idx);
            }

            // Shutdown complete, now launch the new process
            self.launch_process(flock_idx, process_idx)?;
        }

        if poll(Duration::from_millis(100))? {
            match event::read()? {
                Event::Key(k) => match (k.modifiers, k.code) {
                    (KeyModifiers::CONTROL, KeyCode::Char('c'))
                    | (KeyModifiers::NONE, KeyCode::Char('q')) => {
                        self.exit = true;
                    }
                    (KeyModifiers::NONE, KeyCode::Char('j') | KeyCode::Down) => {
                        self.flock_state.next();
                    }
                    (KeyModifiers::NONE, KeyCode::Char('k') | KeyCode::Up) => {
                        self.flock_state.previous();
                    }
                    (KeyModifiers::NONE, KeyCode::Enter) => {
                        if let Some(flock_idx) = self.flock_state.selected {
                            let flock =
                                self.config.flocks.get(flock_idx).ok_or(anyhow!(
                                    "Selected a flock that does not exist anymore"
                                ))?;

                            // Iterate through each process in the flock
                            for process_idx in 0..flock.processes.len() {
                                let should_launch = self
                                    .flock_processes
                                    .get_mut(&flock_idx)
                                    .and_then(|p| p.get_mut(&process_idx))
                                    .map(|existing_process| {
                                        // Check if process has exited
                                        let mut child = existing_process.child.write().unwrap();
                                        match child.try_wait() {
                                            Ok(Some(_)) => true, // Process has exited, relaunch
                                            Ok(None) => false,   // Process still running, skip
                                            Err(_) => true,      // Error checking status, relaunch
                                        }
                                    })
                                    .unwrap_or(true); // Process was never launched

                                if should_launch {
                                    self.launch_process(flock_idx, process_idx)?;
                                }
                            }
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
        let overall_layout = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Percentage(20), Constraint::Fill(1)])
            .split(area);
        let title_style = Style::new().bold();
        let list_builder = ListBuilder::new(|context| {
            // TODO change unwrap
            let mut item =
                FlockItem::new(self.config.flocks.get(context.index).unwrap().to_owned());
            if context.is_selected {
                item.style = item.style.reversed();
            }

            (item, 1)
        });
        ListView::new(list_builder, self.config.flocks.len())
            .block(
                Block::new()
                    .borders(Borders::RIGHT)
                    .title_top("Flocks")
                    .title_style(title_style),
            )
            .render(overall_layout[0], buf, &mut self.flock_state);

        // Display processes for the selected flock
        if let Some(selected_flock_idx) = self.flock_state.selected {
            let flock_process_configs = &self
                .config
                .flocks
                .get(selected_flock_idx)
                .unwrap()
                .processes;

            let overall_layout = Layout::default()
                .direction(Direction::Vertical)
                .constraints(
                    (0..flock_process_configs.len())
                        .map(|_| Constraint::Fill(1))
                        .collect::<Vec<Constraint>>(),
                )
                .split(overall_layout[1]);

            // Render each process panel
            for (process_idx, flock_process_config) in flock_process_configs.iter().enumerate() {
                let layout = overall_layout[process_idx];

                // Check if this flock has processes and if this specific process exists
                let process_option = self
                    .flock_processes
                    .get_mut(&selected_flock_idx)
                    .and_then(|processes| processes.get_mut(&process_idx));

                match process_option {
                    Some(process) => {
                        // Resize PTY and parser to match the layout (accounting for borders)
                        let pty_cols = layout.width.saturating_sub(2);
                        let pty_rows = layout.height.saturating_sub(2);
                        let _ = process.pty_master.resize(PtySize {
                            rows: pty_rows,
                            cols: pty_cols,
                            pixel_width: 0,
                            pixel_height: 0,
                        });
                        process.parser.write().unwrap().set_size(pty_rows, pty_cols);

                        // Get the screen contents from the VT100 parser with colors
                        let parser = process.parser.read().unwrap();
                        let screen = parser.screen();

                        let lines: Vec<Line> = (0..pty_rows)
                            .map(|row| {
                                let mut spans = vec![];
                                let mut current_text = String::new();
                                let mut current_style = Style::default();

                                for col in 0..pty_cols {
                                    let cell = screen.cell(row, col);
                                    if let Some(cell) = cell {
                                        let fg = cell.fgcolor();
                                        let bg = cell.bgcolor();
                                        let is_bold = cell.bold();
                                        let is_italic = cell.italic();
                                        let is_underline = cell.underline();

                                        let mut style = Style::default();

                                        // Convert VT100 colors to Ratatui colors
                                        if let vt100::Color::Idx(idx) = fg {
                                            style = style.fg(ansi_to_ratatui_color(idx));
                                        }
                                        if let vt100::Color::Idx(idx) = bg {
                                            style = style.bg(ansi_to_ratatui_color(idx));
                                        }
                                        if is_bold {
                                            style = style.bold();
                                        }
                                        if is_italic {
                                            style = style.italic();
                                        }
                                        if is_underline {
                                            style = style.underlined();
                                        }

                                        if style != current_style && !current_text.is_empty() {
                                            spans.push(ratatui::text::Span::styled(
                                                current_text.clone(),
                                                current_style,
                                            ));
                                            current_text.clear();
                                        }

                                        current_style = style;
                                        current_text.push_str(&cell.contents());
                                    }
                                }

                                if !current_text.is_empty() {
                                    spans.push(ratatui::text::Span::styled(
                                        current_text,
                                        current_style,
                                    ));
                                }

                                Line::from(spans)
                            })
                            .collect();

                        // Build title with state indicator
                        let state_indicator = match &process.state {
                            ProcessState::Restarting => " [Restarting...]",
                            _ => "",
                        };
                        let title =
                            format!("{}{}", flock_process_config.display_name, state_indicator);

                        Widget::render(
                            Paragraph::new(lines).block(Block::bordered().title(title)),
                            overall_layout[process_idx],
                            buf,
                        );
                    }
                    None => Widget::render(
                        Paragraph::new("").block(
                            Block::bordered().title(flock_process_config.display_name.clone()),
                        ),
                        overall_layout[process_idx],
                        buf,
                    ),
                }
            }
        }
    }
}

// Convert ANSI color index to Ratatui color
fn ansi_to_ratatui_color(idx: u8) -> ratatui::style::Color {
    use ratatui::style::Color;
    match idx {
        0 => Color::Black,
        1 => Color::Red,
        2 => Color::Green,
        3 => Color::Yellow,
        4 => Color::Blue,
        5 => Color::Magenta,
        6 => Color::Cyan,
        7 => Color::Gray,
        8 => Color::DarkGray,
        9 => Color::LightRed,
        10 => Color::LightGreen,
        11 => Color::LightYellow,
        12 => Color::LightBlue,
        13 => Color::LightMagenta,
        14 => Color::LightCyan,
        15 => Color::White,
        _ => Color::Reset,
    }
}

#[derive(Debug, Clone)]
struct FlockItem {
    flock: Flock,
    style: Style,
}

impl FlockItem {
    fn new(flock: Flock) -> Self {
        Self {
            flock,
            style: Style::default(),
        }
    }
}

impl Widget for FlockItem {
    fn render(self, area: Rect, buf: &mut Buffer) {
        Line::from(self.flock.display_name)
            .style(self.style)
            .render(area, buf);
    }
}
