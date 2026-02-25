use ratatui::{
    layout::{Constraint, Direction, Layout},
    widgets::Widget,
};

pub struct SplitListView<T> {
    widgets: Vec<T>,
}

impl<T> SplitListView<T> {
    pub fn new(widgets: Vec<T>) -> Self {
        Self { widgets }
    }
}

impl<T: Widget> Widget for SplitListView<T> {
    fn render(self, area: ratatui::prelude::Rect, buf: &mut ratatui::prelude::Buffer)
    where
        Self: Sized,
    {
        if self.widgets.len() > 0 {
            let overall_layout = Layout::default()
                .direction(Direction::Vertical)
                .constraints(
                    self.widgets
                        .iter()
                        .map(|_| Constraint::Fill(1))
                        .collect::<Vec<_>>(),
                )
                .split(area);
            self.widgets.into_iter().enumerate().for_each(|(i, w)| {
                w.render(overall_layout[i], buf);
            });
        }
    }
}
