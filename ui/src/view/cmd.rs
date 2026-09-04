//! Side effects an `update` (or a controller) may request.

use std::rc::Rc;
use std::time::Duration;

use crate::css::node::Node;
use crate::view::View;
use crate::window::popup::{PopupAnchorPoint, PopupKey, Positioner};

/// A side effect the [`App`](crate::view::app::App) performs on the model's
/// behalf.
///
/// Everything here is deliberately *data*: an `update` returns a `Cmd`, the
/// loop interprets it, and any message it produces is **queued**, never
/// folded re-entrantly (contract §4.7).
pub enum Cmd<Msg> {
    /// Do nothing.
    None,
    /// Do all of these, in order.
    Batch(Vec<Cmd<Msg>>),
    /// Fire once, after `Duration`, on the animation clock.
    After(Duration, Rc<dyn Fn() -> Msg>),
    /// Put text on the clipboard.
    Copy(String),
    /// Read the clipboard; the closure sees `None` on no offer or timeout.
    Paste(Rc<dyn Fn(Option<String>) -> Msg>),
    /// Put text on the primary selection.
    SetPrimary(String),
    /// Read the primary selection.
    Primary(Rc<dyn Fn(Option<String>) -> Msg>),
    /// Open a child popup surface rendering `view`.
    OpenPopup {
        /// Where it hangs off the parent window.
        anchor: PopupAnchorPoint,
        /// Size, anchor, gravity and constraint rules.
        positioner: Positioner,
        /// The popup's own view function.
        view: Rc<dyn Fn() -> View<Msg>>,
    },
    /// Dismiss a popup this app opened.
    ClosePopup(PopupKey),
    /// Move the focus.
    Focus(Node),
    /// Set the toplevel title.
    SetTitle(String),
    /// Minimise the toplevel.
    Minimize,
    /// Toggle the toplevel's maximised state.
    ToggleMaximized,
    /// Ask the compositor to close the window.
    CloseWindow,
    /// Leave the loop.
    Quit,
    /// Run `f` once, on the loop thread, after the fold that produced it.
    ///
    /// `f` **must not block**: the intended body is a channel push to a worker
    /// thread, or a call the app has already proven non-blocking. Anything
    /// whose answer matters comes back through the inbox as a `Msg`, never as
    /// a return value — `Cmd::Task` has none.
    Task(Rc<dyn Fn()>),
}

#[allow(
    clippy::derivable_impls,
    reason = "a #[derive(Default)] would add a `Msg: Default` bound this one does not need"
)]
impl<Msg> Default for Cmd<Msg> {
    fn default() -> Self {
        Cmd::None
    }
}

impl<Msg> std::fmt::Debug for Cmd<Msg> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Cmd::None => f.write_str("None"),
            Cmd::Batch(list) => f.debug_tuple("Batch").field(list).finish(),
            Cmd::After(d, _) => write!(f, "After({d:?}, ..)"),
            Cmd::Copy(text) => write!(f, "Copy({} bytes)", text.len()),
            Cmd::Paste(_) => f.write_str("Paste(..)"),
            Cmd::SetPrimary(text) => write!(f, "SetPrimary({} bytes)", text.len()),
            Cmd::Primary(_) => f.write_str("Primary(..)"),
            Cmd::OpenPopup { positioner, .. } => f
                .debug_struct("OpenPopup")
                .field("positioner", positioner)
                .finish_non_exhaustive(),
            Cmd::ClosePopup(key) => f.debug_tuple("ClosePopup").field(key).finish(),
            Cmd::Focus(node) => write!(f, "Focus({})", node.name()),
            Cmd::SetTitle(title) => f.debug_tuple("SetTitle").field(title).finish(),
            Cmd::Minimize => f.write_str("Minimize"),
            Cmd::ToggleMaximized => f.write_str("ToggleMaximized"),
            Cmd::CloseWindow => f.write_str("CloseWindow"),
            Cmd::Quit => f.write_str("Quit"),
            Cmd::Task(_) => f.write_str("Task(..)"),
        }
    }
}

impl<Msg> Cmd<Msg> {
    /// Flatten nested [`Cmd::Batch`]es into one list, dropping every
    /// [`Cmd::None`]. The loop interprets the flat list, so a deeply nested
    /// batch from composed `update`s costs one pass, not a recursion.
    #[must_use]
    pub fn flatten(self) -> Vec<Cmd<Msg>> {
        let mut out = Vec::new();
        let mut stack = vec![self];
        while let Some(cmd) = stack.pop() {
            match cmd {
                Cmd::None => {}
                // Pushed in reverse so `pop` yields them in written order.
                Cmd::Batch(list) => stack.extend(list.into_iter().rev()),
                other => out.push(other),
            }
        }
        out
    }

    /// Whether this command asks for nothing at all.
    #[must_use]
    pub fn is_none(&self) -> bool {
        match self {
            Cmd::None => true,
            Cmd::Batch(list) => list.iter().all(Cmd::is_none),
            _ => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Debug, Clone, PartialEq)]
    enum Msg {
        A,
        B,
        C,
    }

    #[test]
    fn flatten_preserves_written_order_and_drops_nones() {
        let cmd: Cmd<Msg> = Cmd::Batch(vec![
            Cmd::None,
            Cmd::Copy("a".into()),
            Cmd::Batch(vec![Cmd::SetTitle("b".into()), Cmd::None, Cmd::Quit]),
            Cmd::Minimize,
        ]);
        let flat = cmd.flatten();
        let names: Vec<String> = flat.iter().map(|c| format!("{c:?}")).collect();
        assert_eq!(
            names,
            vec![
                "Copy(1 bytes)".to_owned(),
                "SetTitle(\"b\")".to_owned(),
                "Quit".to_owned(),
                "Minimize".to_owned(),
            ]
        );
    }

    #[test]
    fn a_batch_of_nothing_is_nothing() {
        let cmd: Cmd<Msg> = Cmd::Batch(vec![Cmd::None, Cmd::Batch(vec![Cmd::None])]);
        assert!(cmd.is_none());
        assert!(cmd.flatten().is_empty());
    }

    #[test]
    fn an_after_command_keeps_its_message_thunk() {
        let cmd: Cmd<Msg> = Cmd::After(Duration::from_millis(5), Rc::new(|| Msg::A));
        let flat = cmd.flatten();
        assert_eq!(flat.len(), 1);
        let Cmd::After(d, f) = &flat[0] else {
            panic!("After was rewritten by flatten");
        };
        assert_eq!(*d, Duration::from_millis(5));
        assert_eq!(f(), Msg::A);
        // The other two variants exist and are distinct.
        assert_ne!(Msg::B, Msg::C);
    }

    #[test]
    fn a_task_is_a_flatten_leaf_and_prints_opaquely() {
        // mutation: give `Cmd::Task` a `Batch`-like arm in `flatten`; it
        // disappears from the flat list and this fails.
        let ran = std::rc::Rc::new(std::cell::Cell::new(false));
        let flag = std::rc::Rc::clone(&ran);
        let cmd: Cmd<Msg> = Cmd::Batch(vec![
            Cmd::Task(std::rc::Rc::new(move || flag.set(true))),
            Cmd::Quit,
        ]);
        assert!(!cmd.is_none());
        let flat = cmd.flatten();
        assert_eq!(flat.len(), 2);
        assert_eq!(format!("{:?}", flat[0]), "Task(..)");
        assert!(!ran.get(), "flatten must not run the task");
    }
}
