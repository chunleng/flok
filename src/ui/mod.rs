mod components;

use std::time::Duration;

use crossterm::event::{self, Event, KeyCode, KeyModifiers};
use ratatui::{
    DefaultTerminal, Frame,
    buffer::Buffer,
    crossterm::event::poll,
    layout::{Constraint, Direction, Layout, Rect},
    prelude::*,
    widgets::Widget,
};

use crate::state::AppState;
use crate::ui::components::lists::{SideListView, SplitListView};
use crate::utils::process::ProcessStatus;
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
    state: AppState,
}

impl App {
    fn new(config: AppConfig) -> Result<Self, anyhow::Error> {
        Ok(Self {
            exit: false,
            state: AppState::new(config.clone()),
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
                        self.state.next_item();
                    }
                    (KeyModifiers::NONE, KeyCode::Char('k') | KeyCode::Up) => {
                        self.state.previous_item();
                    }
                    (KeyModifiers::NONE, KeyCode::Enter) => {
                        self.state.select();
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
        match &mut self.state {
            AppState::Main(state, global_state) => {
                SideListView::new(
                    "Flocks".to_string(),
                    global_state
                        .flocks
                        .iter()
                        .map(|f| f.display_name.to_owned())
                        .collect(),
                )
                .render(sidebar_area, buf, &mut state.active_flock);

                let mut widgets = Vec::new();
                global_state
                    .flocks
                    .get(state.active_flock)
                    .unwrap()
                    .processes
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
}

