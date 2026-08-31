//! `GtkCalendar` — `Kind::Calendar`, CSS node `calendar`, always `.view`.
//!
//! ```text
//! calendar.view
//! ├── header
//! │   ├── button
//! │   ├── stack.month
//! │   ├── button
//! │   ├── button
//! │   ├── label.year
//! │   ╰── button
//! ╰── grid
//!     ╰── label[.day-name][.week-number][.day-number][.other-month][.today]
//! ```
//!
//! GTK lays the day labels out in a grid; `layout::Container::Grid` is P6's, so
//! P5 nests one row box per week under `grid`. The *CSS node tree* the fixture
//! pins is unaffected: `grid`'s children are still the day labels, because the
//! row boxes are layout-only and are not CSS nodes — they are created as
//! `Container::Box` children of a node that is itself the `grid`, one row at a
//! time, by appending the labels directly and letting P6's Grid variant take
//! over the placement when it lands.
//!
//! Reconciliation: `View::on_date_selected` already exists as a generic
//! `on_<eventkind>` setter (`view::builders`, contract deviation D13) firing
//! `Handler::Text` with an ISO-8601 `YYYY-MM-DD` payload — `Handler` has no
//! `(i32, u32, u32)` variant, so the plan's own §5 shape was folded into
//! `Text` before this task ever ran. The task text as written re-declares
//! `on_date_selected` on `CalendarExt` with a `usize`-day/`Handler::Index`
//! shape; that would collide with the inherent method (which always wins
//! method resolution, silently stranding the trait version) and contradicts
//! the already-binding D13. `CalendarExt` therefore omits `on_date_selected`
//! and the controller fires `Handler::Text` with the ISO date instead.

use crate::css::node::{Node, PseudoStates};
use crate::layout::Rect;
use crate::view::controller::{Controller, Event, EventCx};
use crate::view::{BuildCx, EventKind, Kind, Prop, PropName, Props, View};
use crate::widgets::{PointerState, local_rect, shift_event};

/// A `GtkCalendar` showing `year`/`month` with `day` selected.
#[must_use]
pub fn calendar<Msg: Clone + 'static>(year: i32, month: u32, day: u32) -> View<Msg> {
    View::new(Kind::Calendar)
        .prop(PropName::Row, Prop::Int(i64::from(year)))
        .prop(PropName::Column, Prop::Int(i64::from(month)))
        .prop(PropName::Value, Prop::Float(f64::from(day)))
}

/// `GtkCalendar`'s own setters.
///
/// `on_date_selected` is not here: it is already the generic
/// `View::on_date_selected` (`view::builders`, contract D13), which fires
/// `Handler::Text` with an ISO-8601 `YYYY-MM-DD` payload.
pub trait CalendarExt<Msg>: Sized {
    /// `GtkCalendar:show-day-names`.
    fn show_day_names(self, on: bool) -> Self;
    /// `GtkCalendar:show-heading`.
    fn show_heading(self, on: bool) -> Self;
    /// `GtkCalendar:show-week-numbers`.
    fn show_week_numbers(self, on: bool) -> Self;
    /// `gtk_calendar_mark_day`. Repeated calls accumulate into a 31-bit mask.
    fn mark_day(self, day: u32) -> Self;
}

impl<Msg: Clone + 'static> CalendarExt<Msg> for View<Msg> {
    fn show_day_names(self, on: bool) -> Self {
        self.prop(PropName::ShowText, Prop::Bool(on))
    }
    fn show_heading(self, on: bool) -> Self {
        self.prop(PropName::Title, Prop::Bool(on))
    }
    fn show_week_numbers(self, on: bool) -> Self {
        self.prop(PropName::ShowSeparators, Prop::Bool(on))
    }
    fn mark_day(self, day: u32) -> Self {
        let previous = match self.props.get(PropName::Detail) {
            Some(Prop::Int(mask)) => *mask,
            _ => 0,
        };
        let bit = 1i64 << day.clamp(1, 31);
        self.prop(PropName::Detail, Prop::Int(previous | bit))
    }
}

/// `Kind::Calendar`'s controller.
pub struct CalendarC {
    /// The `(year, month)` currently displayed.
    pub shown: (i32, u32),
    /// The selected `(year, month, day)`.
    pub selected: (i32, u32, u32),
    /// Marked days, one bit per day of the month.
    pub marks: u32,
    /// The `header` subnode.
    pub header: Node,
    /// The `grid` subnode.
    pub grid: Node,
    /// One `label` per day cell, ascending.
    pub day_nodes: Vec<Node>,
    pointer: PointerState,
}

impl CalendarC {
    /// Days in `month` of `year`, Gregorian. An out-of-range month clamps to
    /// December's 31 rather than panicking — the month arrives from a model.
    #[must_use]
    pub fn days_in_month(year: i32, month: u32) -> u32 {
        match month {
            1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
            4 | 6 | 9 | 11 => 30,
            2 => {
                let leap = (year % 4 == 0 && year % 100 != 0) || year % 400 == 0;
                if leap { 29 } else { 28 }
            }
            _ => 31,
        }
    }

    /// Weekday of the 1st of `month`, 0 = Monday, by Zeller's congruence.
    #[must_use]
    pub fn first_weekday(year: i32, month: u32) -> u32 {
        let (mut y, m) = (year, month.clamp(1, 12));
        let m = if m < 3 {
            y -= 1;
            m + 12
        } else {
            m
        };
        let k = y.rem_euclid(100);
        let j = y.div_euclid(100);
        // Zeller yields 0 = Saturday; shift to 0 = Monday.
        let h = (1 + (13 * (m as i32 + 1)) / 5 + k + k / 4 + j / 4 + 5 * j).rem_euclid(7);
        u32::try_from((h + 5).rem_euclid(7)).unwrap_or(0)
    }

    /// Rebuild the day grid for `shown`.
    fn rebuild(&mut self) {
        for node in self.day_nodes.drain(..) {
            node.detach();
        }
        let days = Self::days_in_month(self.shown.0, self.shown.1);
        for day in 1..=days {
            let mut classes: Vec<&str> = vec!["day-number"];
            if (self.marks >> day) & 1 == 1 {
                classes.push("today");
            }
            let node = Node::with_classes("label", &classes);
            node.set_state(
                PseudoStates::SELECTED,
                self.selected == (self.shown.0, self.shown.1, day),
            );
            self.grid.append_child(&node);
            self.day_nodes.push(node);
        }
    }
}

impl<Msg: Clone + 'static> Controller<Msg> for CalendarC {
    fn kind(&self) -> Kind {
        Kind::Calendar
    }

    fn build(node: &Node, props: &Props, _cx: &mut BuildCx<'_>) -> Self {
        node.add_class("view");
        let header = Node::new("header");
        node.append_child(&header);
        // GTK's header is button, stack.month, button, button, label.year,
        // button — six children, in that order.
        header.append_child(&Node::new("button"));
        header.append_child(&Node::with_classes("stack", &["month"]));
        header.append_child(&Node::new("button"));
        header.append_child(&Node::new("button"));
        header.append_child(&Node::with_classes("label", &["year"]));
        header.append_child(&Node::new("button"));
        let grid = Node::new("grid");
        node.append_child(&grid);

        let year = i32::try_from(props.int(PropName::Row, 1970)).unwrap_or(1970);
        let month = u32::try_from(props.int(PropName::Column, 1))
            .unwrap_or(1)
            .clamp(1, 12);
        let day = props.float(PropName::Value, 1.0).clamp(1.0, 31.0) as u32;
        let mut this = CalendarC {
            shown: (year, month),
            selected: (year, month, day),
            marks: u32::try_from(props.int(PropName::Detail, 0)).unwrap_or(0),
            header,
            grid,
            day_nodes: Vec::new(),
            pointer: PointerState::default(),
        };
        this.rebuild();
        this
    }

    fn set_prop(&mut self, _node: &Node, name: PropName, value: &Prop, _cx: &mut BuildCx<'_>) {
        match (name, value) {
            (PropName::Row, Prop::Int(y)) => {
                self.shown.0 = i32::try_from(*y).unwrap_or(self.shown.0);
                self.selected.0 = self.shown.0;
            }
            (PropName::Column, Prop::Int(m)) => {
                self.shown.1 = u32::try_from(*m).unwrap_or(self.shown.1).clamp(1, 12);
                self.selected.1 = self.shown.1;
            }
            (PropName::Value, Prop::Float(d)) => {
                self.selected.2 = d.clamp(1.0, 31.0) as u32;
            }
            (PropName::Detail, Prop::Int(mask)) => {
                self.marks = u32::try_from(*mask).unwrap_or(0);
            }
            _ => return,
        }
        self.rebuild();
    }

    fn on_event(&mut self, ev: &Event, cx: &mut EventCx<'_, Msg>) -> Vec<Msg> {
        for (index, day_node) in self.day_nodes.iter().enumerate() {
            let Some(rect) = local_rect(cx.tree, cx.node, day_node) else {
                continue;
            };
            let shifted = shift_event(ev, rect);
            if self.pointer.observe(
                day_node,
                &shifted,
                Some(Rect::new(0.0, 0.0, rect.width, rect.height)),
            ) {
                cx.handled = true;
                let day = index as u32 + 1;
                let iso = format!("{:04}-{:02}-{:02}", self.shown.0, self.shown.1, day);
                if let Some(msg) = cx.handlers.fire_text(EventKind::DateSelected, &iso) {
                    return vec![msg];
                }
                return Vec::new();
            }
        }
        Vec::new()
    }
}
