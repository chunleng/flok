use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Style, Stylize},
    widgets::{Block, Borders, List, ListItem, ListState, StatefulWidget},
};

use crate::ui::components::texts::TITLE_STYLE;

pub struct SideListView<'a> {
    title: String,
    items: Vec<ListItem<'a>>,
}
impl<'a> SideListView<'a> {
    pub fn new(title: String, items: Vec<String>) -> Self {
        Self {
            title,
            items: items
                .iter()
                .map(|item| ListItem::new(item.to_owned()))
                .collect(),
        }
    }
}
impl<'a> StatefulWidget for SideListView<'a> {
    type State = ListState;
    fn render(self, area: Rect, buf: &mut Buffer, state: &mut Self::State) {
        StatefulWidget::render(
            List::new(self.items)
                .block(
                    Block::new()
                        .borders(Borders::RIGHT)
                        .title_top(self.title)
                        .title_style(*TITLE_STYLE),
                )
                .highlight_style(Style::default().reversed()),
            area,
            buf,
            state,
        );
    }
}
