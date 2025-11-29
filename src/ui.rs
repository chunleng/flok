use std::io::Write;
use std::{
    cell::RefCell,
    collections::HashMap,
    io::Read,
    process::{Child, Command, Stdio},
    rc::Rc,
    sync::{Arc, RwLock},
    time::Duration,
};

use anyhow::anyhow;
use crossterm::event::{self, Event, KeyCode, KeyModifiers};
use ratatui::widgets::Paragraph;
use ratatui::{
    DefaultTerminal, Frame,
    buffer::Buffer,
    crossterm::event::poll,
    layout::{Constraint, Direction, Layout, Rect},
    prelude::*,
    style::{Style, Stylize},
    text::Line,
    widgets::{Block, Borders, Widget},
};
use tempfile::NamedTempFile;
use tui_widget_list::{ListBuilder, ListState, ListView};

use crate::{
    Config, Flock,
    error::{FlokProgramError, FlokProgramExecutionError},
};

pub fn run(config: Config) -> Result<(), FlokProgramError> {
    let mut terminal = ratatui::init();
    let app_result = App::new(config).run(&mut terminal);
    ratatui::restore();

    app_result
}

struct Process {
    child: Child,
    logs: Arc<RwLock<Vec<String>>>,
}

struct App {
    exit: bool,
    config: Config,
    flock_state: ListState,
    flock_processes: HashMap<usize, HashMap<usize, Process>>,
}

impl App {
    fn new(config: Config) -> Self {
        let mut flock_state = ListState::default();
        flock_state.select(Some(0));
        Self {
            exit: false,
            config,
            flock_state,
            flock_processes: HashMap::new(),
        }
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

    fn handle_event(&mut self) -> Result<(), FlokProgramExecutionError> {
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
                            let processes = self
                                .flock_processes
                                .entry(flock_idx)
                                .or_insert_with(HashMap::new);

                            let flock =
                                self.config.flocks.get(flock_idx).ok_or(anyhow!(
                                    "Selected a flock that does not exist anymore"
                                ))?;

                            // Iterate through each process in the flock
                            for (process_idx, flock_process) in flock.processes.iter().enumerate() {
                                let should_launch = match processes.get_mut(&process_idx) {
                                    Some(existing_process) => {
                                        match existing_process.child.try_wait() {
                                            Ok(Some(_)) => true, // Process has exited, relaunch
                                            Ok(None) => false,   // Process still running, skip
                                            Err(_) => true,      // Error checking status, relaunch
                                        }
                                    }
                                    None => true, // Process was never launched
                                };

                                if should_launch {
                                    // Launch the process
                                    let mut script = NamedTempFile::new()?;
                                    let script_path = script.path().display().to_string();
                                    writeln!(script, "{}", flock_process.command)?;
                                    let _ = script.persist(script_path.clone());
                                    // TODO It's using zsh because my terminal is zsh. To make this
                                    // use the login shell that the program is running on instead.
                                    let mut child = Command::new("zsh")
                                        .arg(script_path)
                                        .stdout(Stdio::piped())
                                        .spawn()?;

                                    let stdout = child
                                        .stdout
                                        .take()
                                        .ok_or(anyhow!("Unable to get stdout from command"))?;

                                    let logs = Arc::new(RwLock::new(vec![]));
                                    let logs_clone = logs.clone();

                                    std::thread::spawn(move || {
                                        let stdout = Rc::new(RefCell::new(stdout));
                                        loop {
                                            let mut buffer = [0; 20000];
                                            let bytes_read =
                                                stdout.borrow_mut().read(&mut buffer)?;
                                            if bytes_read == 0 {
                                                return Ok::<(), std::io::Error>(());
                                            }
                                            buffer.split(|char| char == &b'\n').for_each(|buf| {
                                                if !buf.iter().all(|c| c == &b'\0') {
                                                    logs_clone.write().unwrap().push(
                                                        String::from_utf8_lossy(&buf).to_string(),
                                                    );
                                                }
                                            })
                                        }
                                    });

                                    processes.insert(process_idx, Process { child, logs });
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
                    .get(&selected_flock_idx)
                    .and_then(|processes| processes.get(&process_idx));

                match process_option {
                    Some(process) => {
                        let all_logs = process.logs.read().unwrap();
                        let display_logs =
                            &all_logs[all_logs.len().saturating_sub((layout.height - 2).into())..];
                        Widget::render(
                            Paragraph::new(
                                display_logs
                                    .iter()
                                    .map(|x| Line::from(x.clone()))
                                    .collect::<Vec<Line>>(),
                            )
                            .block(
                                Block::bordered().title(flock_process_config.display_name.clone()),
                            ),
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
