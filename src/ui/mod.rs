mod components;

use std::time::Duration;

use crossterm::event::{self, Event, KeyCode, KeyModifiers};
use ratatui::widgets::ListState;
use ratatui::{
    DefaultTerminal, Frame,
    buffer::Buffer,
    crossterm::event::poll,
    layout::{Constraint, Direction, Layout, Rect},
    prelude::*,
    widgets::Widget,
};

use crate::ui::components::lists::{SideListView, SplitListView};
use crate::utils::process::{ProcessState, ProcessStatus};
use crate::{
    config::AppConfig,
    error::{FlokProgramError, FlokProgramExecutionError, FlokProgramInitError},
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

        Ok(Self {
            exit: false,
            config,
            flock_state,
            flock_processes,
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

    fn handle_event(&mut self) -> Result<(), FlokProgramExecutionError> {
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
                    if let Ok(status) = state.status.read() {
                        match *status {
                            ProcessStatus::Running(ref process) => {
                                // Build title with state indicator
                                let state_indicator = match &process.status {
                                    ProcessRunningStatus::Restarting => " [Restarting...]",
                                    _ => "",
                                };
                                let title = format!(
                                    "{}{}",
                                    state.process_config.display_name, state_indicator
                                );

                                widgets.push(AutoFillPty::new(
                                    process.pty_master.clone(),
                                    process.parser.clone(),
                                    title,
                                ));
                            }
                            _ => {}
                        }
                    }
                });

            SplitListView::new(widgets).render(main_area, buf)
        }
    }
}
