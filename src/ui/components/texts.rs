use std::cell::LazyCell;

use ratatui::style::{Style, Stylize};

pub const TITLE_STYLE: LazyCell<Style> = LazyCell::new(|| Style::new().bold());
