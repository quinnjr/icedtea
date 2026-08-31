//! Keyed reconciliation of a `View` description tree into retained
//! [`Node`](crate::css::node::Node)s.

use std::rc::Rc;

use crate::anim::Clock;
use crate::css::cascade::CompiledSheet;
use crate::css::computed::ResolveEnv;
use crate::icons::IconTheme;
use crate::text::FontDatabase;
use crate::view::PropName;

/// Everything building or updating a controller needs, threaded down the
/// whole reconcile.
pub struct BuildCx<'a> {
    /// The compiled theme, for `@keyframes` lookup and colour resolution.
    pub sheet: &'a CompiledSheet,
    /// Font matching, loading and shaping.
    pub fonts: &'a mut FontDatabase,
    /// Icon lookup and rendering.
    pub icons: &'a mut IconTheme,
    /// The animation clock; `ManualClock` under test.
    pub clock: &'a Rc<dyn Clock>,
    /// DPI and root font size.
    pub env: &'a ResolveEnv,
}

impl std::fmt::Debug for BuildCx<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BuildCx")
            .field("rules", &self.sheet.rules.len())
            .field("icon_theme", &self.icons.name())
            .field("env", &self.env)
            .finish_non_exhaustive()
    }
}

/// One reconciliation step, at one level of the tree.
///
/// The list a `reconcile` call returns covers **that level only**: a
/// `Recurse` says the child at `index` was descended into, and the ops the
/// recursion produced are not spliced into the parent's list.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Op {
    /// A new child was built and inserted at `index`.
    Insert {
        /// Position in the new child list.
        index: usize,
    },
    /// The child that was at `index` was dropped.
    Remove {
        /// Position in the *previous* child list.
        index: usize,
    },
    /// A kept child moved.
    Move {
        /// Position in the previous child list.
        from: usize,
        /// Position in the new child list.
        to: usize,
    },
    /// One property of the child at `index` changed.
    SetProp {
        /// Position in the new child list.
        index: usize,
        /// Which property.
        name: PropName,
    },
    /// The child at `index` took this frame's handler set.
    SetHandlers {
        /// Position in the new child list.
        index: usize,
    },
    /// The child at `index` was reconciled recursively.
    Recurse {
        /// Position in the new child list.
        index: usize,
    },
}
