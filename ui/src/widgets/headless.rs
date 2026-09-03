//! Building a widget with no Wayland connection and no `App`.
//!
//! Split out of `widgets/mod.rs` verbatim; every item is re-exported from
//! [`crate::widgets`], which is the path every caller uses.

use std::rc::Rc;

use crate::css::node::Node;
use crate::view::controller::Controller;
use crate::view::{BuildCx, Kind, Props};

use super::build_controller;

/// Everything `Controller::build` needs, with no Wayland connection and no
/// application: the bundled Adwaita light sheet, a probe-only font database,
/// a rootless icon theme and a `ManualClock` at zero.
pub struct Headless {
    sheet: crate::css::cascade::CompiledSheet,
    fonts: crate::text::FontDatabase,
    icons: crate::icons::IconTheme,
    clock: Rc<dyn crate::anim::Clock>,
    env: crate::css::computed::ResolveEnv,
    /// Backing storage for [`Headless::event_cx`]'s `tree`/`styles`/`focus`/
    /// `clipboard` fields -- empty and unsynced, since no widget task's
    /// `on_event` test hits an allocation-dependent path.
    tree: crate::layout::LayoutTree,
    styles: crate::view::StyleMap,
    focus: crate::window::focus::FocusRing,
    clipboard: crate::window::selection::Clipboard,
}

impl Headless {
    /// A context over `BUNDLED_ADWAITA_LIGHT`.
    #[must_use]
    pub fn new() -> Self {
        let sheet = crate::css::cascade::CompiledSheet::compile(crate::BUNDLED_ADWAITA_LIGHT);
        Self {
            sheet,
            fonts: crate::text::FontDatabase::new(),
            // There is no `IconTheme::empty()`; `node_tree_of` below already
            // stands up a rootless "hicolor" theme for the same hermetic
            // purpose, so this reuses that rather than adding a second way to
            // spell "no real icons".
            icons: crate::icons::IconTheme::with_name_and_roots("hicolor", Vec::new()),
            clock: Rc::new(crate::anim::ManualClock::new()),
            env: crate::css::computed::ResolveEnv::default(),
            tree: crate::layout::LayoutTree::new(),
            styles: crate::view::StyleMap::new(),
            focus: <crate::window::focus::FocusRing as Default>::default(),
            clipboard: crate::window::selection::Clipboard::offscreen(),
        }
    }

    /// Borrow it as a `BuildCx`.
    pub fn cx(&mut self) -> BuildCx<'_> {
        BuildCx {
            sheet: &self.sheet,
            fonts: &mut self.fonts,
            icons: &mut self.icons,
            clock: &self.clock,
            env: &self.env,
        }
    }

    /// An `EventCx` over `node`, with an empty focus ring and clipboard.
    ///
    /// `handlers` and `cmds` are the two fields an `EventCx` carries that
    /// depend on `Msg`, which `Headless` itself is not generic over; each
    /// call leaks a fresh, empty pair for them (`Box::leak`, never freed).
    /// `Headless` is test/dev-only scaffolding built once per test and
    /// dropped at its end, so the leak is a handful of bytes per call, not a
    /// growing one -- the same trade the rest of this struct already makes
    /// by never tearing down its font/icon caches either.
    pub fn event_cx<'a, Msg: 'static>(
        &'a mut self,
        node: &'a Node,
    ) -> crate::view::EventCx<'a, Msg> {
        let handlers: &crate::view::Handlers<Msg> =
            Box::leak(Box::new(crate::view::Handlers::default()));
        let cmds: &mut Vec<crate::view::Cmd<Msg>> = Box::leak(Box::new(Vec::new()));
        crate::view::EventCx {
            node,
            handlers,
            tree: &self.tree,
            styles: &self.styles,
            focus: &mut self.focus,
            clipboard: &mut self.clipboard,
            icons: &mut self.icons,
            fonts: &mut self.fonts,
            clock: &self.clock,
            env: &self.env,
            cmds,
            phase: crate::view::Phase::Target,
            handled: false,
        }
    }

    /// [`Headless::event_cx`], with `populate` given a chance to register
    /// handlers first -- a controller's `on_event` only emits a `Msg` when
    /// [`crate::view::Handlers`] actually holds a handler for the
    /// [`crate::view::EventKind`] it fires, which the empty set `event_cx`
    /// leaks never does.
    pub fn event_cx_with_handlers<'a, Msg: Clone + 'static>(
        &'a mut self,
        node: &'a Node,
        populate: impl FnOnce(&mut crate::view::Handlers<Msg>),
    ) -> crate::view::EventCx<'a, Msg> {
        let mut handlers = crate::view::Handlers::default();
        populate(&mut handlers);
        let handlers: &crate::view::Handlers<Msg> = Box::leak(Box::new(handlers));
        let cmds: &mut Vec<crate::view::Cmd<Msg>> = Box::leak(Box::new(Vec::new()));
        crate::view::EventCx {
            node,
            handlers,
            tree: &self.tree,
            styles: &self.styles,
            focus: &mut self.focus,
            clipboard: &mut self.clipboard,
            icons: &mut self.icons,
            fonts: &mut self.fonts,
            clock: &self.clock,
            env: &self.env,
            cmds,
            phase: crate::view::Phase::Target,
            handled: false,
        }
    }

    /// A synthetic, already-pressed key event for keysym `name` (an
    /// xkbcommon keysym name, e.g. `"Right"`, `"Home"`), with no modifiers.
    ///
    /// Reconciliation: the task text describes this as going through
    /// `Keymap::vendored_us().key_by_name(name)`, but neither exists --
    /// `window::keyboard::Keymap` (P3's file, off limits to P6 except its
    /// own registration lines) translates a *keycode* it already holds
    /// (`Keymap::translate`) and has no reverse keysym-to-keycode lookup to
    /// build one on. `KeyEvent`'s fields are all public, so this builds one
    /// directly from `xkb::keysym_from_name` instead of inventing a P3 API
    /// this task cannot add.
    ///
    /// Takes no `&self`: it needs none, and `event_cx`'s returned `EventCx`
    /// already holds `Headless` mutably borrowed for as long as a test keeps
    /// using it, which a `&self`/`&mut self` receiver here would collide
    /// with on every call interleaved with `on_event` the way the widget
    /// tests that use both actually call them.
    #[must_use]
    pub fn key(name: &str) -> crate::window::keyboard::KeyEvent {
        let keysym = xkbcommon::xkb::keysym_from_name(name, xkbcommon::xkb::KEYSYM_NO_FLAGS);
        crate::window::keyboard::KeyEvent {
            keycode: 0,
            keysym,
            utf8: None,
            mods: crate::window::keyboard::Mods::empty(),
            consumed: crate::window::keyboard::Mods::empty(),
            pressed: true,
            repeat: false,
            serial: 0,
            time_ms: 0,
        }
    }

    /// [`Headless::key`], with `mods` (xkbcommon `Mods` variant names --
    /// `"Control"`, `"Alt"`, `"Shift"`, `"Logo"`, `"Lock"`, `"NumLock"`) held.
    ///
    /// Reconciliation: introduced for Task 13's `NotebookC` key-nav test,
    /// which needs `Ctrl+PageDown` and `Alt+<digit>`; [`Headless::key`] takes
    /// no modifiers and the task text's `Keymap::key_with_mods` does not
    /// exist for the same reason `key`'s own doc comment gives for itself.
    #[must_use]
    pub fn key_with_mods(name: &str, mods: &[&str]) -> crate::window::keyboard::KeyEvent {
        let mut event = Self::key(name);
        for m in mods {
            event.mods |= match *m {
                "Shift" => crate::window::keyboard::Mods::SHIFT,
                "Control" => crate::window::keyboard::Mods::CTRL,
                "Alt" => crate::window::keyboard::Mods::ALT,
                "Logo" => crate::window::keyboard::Mods::LOGO,
                "Lock" => crate::window::keyboard::Mods::CAPS,
                "NumLock" => crate::window::keyboard::Mods::NUM,
                _ => crate::window::keyboard::Mods::empty(),
            };
        }
        event
    }
}

impl Headless {
    /// Lay `node`'s children out as fixed-height rows stacked in a column,
    /// for a hit-testing test that needs real geometry without wanting to
    /// stand up a full CSS layout pass.
    ///
    /// Runs a real (if minimal) layout: [`crate::layout::LayoutTree::sync`],
    /// each child styled as a [`crate::layout::Container::Leaf`] so its
    /// size comes only from the [`crate::layout::FixedMeasure`] this hands
    /// `compute`, then [`crate::layout::LayoutTree::compute`]. The result
    /// lands in `self.tree`, the same tree `Headless::event_cx`'s returned
    /// `EventCx` borrows -- so a test that also wants an `EventCx` must call
    /// this first: `event_cx`/`event_cx_with_handlers` take `&mut self` for
    /// as long as the `EventCx` they return is alive, and a second call on
    /// `self` (this one included) would not borrow-check afterwards.
    pub fn place_rows(&mut self, node: &Node, row_height: f32) {
        self.tree.sync(node).expect("place_rows: sync");
        self.tree.set_container(
            node,
            crate::layout::Container::Box {
                direction: crate::layout::BoxDirection::Column,
            },
        );
        let style = crate::css::computed::ComputedStyle::initial(&self.env);
        for child in node.children() {
            self.tree
                .set_style(&child, &style, crate::layout::Container::Leaf, &self.env);
        }
        let mut measure = crate::layout::FixedMeasure(taffy::Size {
            width: 200.0,
            height: row_height,
        });
        self.tree
            .compute(
                node,
                taffy::Size {
                    width: taffy::AvailableSpace::Definite(200.0),
                    height: taffy::AvailableSpace::MaxContent,
                },
                &mut measure,
            )
            .expect("place_rows: compute");
    }

    /// Lay a `ColumnView`'s `header` node's children out as a row of
    /// fixed-`width` columns, for a click test that needs to know which
    /// column a point landed in without standing up a full CSS layout pass.
    ///
    /// [`Headless::place_rows`]'s own approach, over a different shape: the
    /// passed `node` becomes a column box (so a header sits above whatever
    /// follows it), its `header` child (if any) becomes a row box, and every
    /// leaf -- the header's own columns, plus any other direct child of
    /// `node` (e.g. the embedded `listview`) -- gets the same fixed
    /// `(width, 30px)` measure. Same caveat as `place_rows`: this borrows
    /// `self.tree` for as long as the `EventCx` a later `event_cx` call
    /// returns is alive, so a test that wants both must call this first.
    pub fn place_columns(&mut self, node: &Node, width: f32) {
        self.tree.sync(node).expect("place_columns: sync");
        self.tree.set_container(
            node,
            crate::layout::Container::Box {
                direction: crate::layout::BoxDirection::Column,
            },
        );
        let header = node.children().into_iter().find(|c| &*c.name() == "header");
        if let Some(header) = &header {
            self.tree.set_container(
                header,
                crate::layout::Container::Box {
                    direction: crate::layout::BoxDirection::Row,
                },
            );
        }
        let style = crate::css::computed::ComputedStyle::initial(&self.env);
        for child in node.children() {
            if header.as_ref().is_some_and(|h| h.ptr_eq(&child)) {
                for column in child.children() {
                    self.tree
                        .set_style(&column, &style, crate::layout::Container::Leaf, &self.env);
                }
            } else {
                self.tree
                    .set_style(&child, &style, crate::layout::Container::Leaf, &self.env);
            }
        }
        let mut measure = crate::layout::FixedMeasure(taffy::Size {
            width,
            height: 30.0,
        });
        self.tree
            .compute(
                node,
                taffy::Size {
                    width: taffy::AvailableSpace::MaxContent,
                    height: taffy::AvailableSpace::MaxContent,
                },
                &mut measure,
            )
            .expect("place_columns: compute");
    }

    /// Lay `node`'s own direct children out as a row of fixed-`width`
    /// columns.
    ///
    /// [`Headless::place_columns`]'s own approach, minus the nested
    /// `header` indirection: that helper only puts its *header* child into
    /// a row, leaving `node` itself a column (right for `ColumnView`, whose
    /// header sits above a `listview`) -- a widget whose own direct
    /// children already sit side by side (`PopoverMenuBar`'s `item`s under
    /// `menubar`, no wrapper) needs `node` itself in `Row`. Same borrow
    /// caveat: call this before `Headless::event_cx`/`event_cx_with_handlers`.
    pub fn place_row(&mut self, node: &Node, width: f32) {
        self.tree.sync(node).expect("place_row: sync");
        self.tree.set_container(
            node,
            crate::layout::Container::Box {
                direction: crate::layout::BoxDirection::Row,
            },
        );
        let style = crate::css::computed::ComputedStyle::initial(&self.env);
        for child in node.children() {
            self.tree
                .set_style(&child, &style, crate::layout::Container::Leaf, &self.env);
        }
        let mut measure = crate::layout::FixedMeasure(taffy::Size {
            width,
            height: 30.0,
        });
        self.tree
            .compute(
                node,
                taffy::Size {
                    width: taffy::AvailableSpace::MaxContent,
                    height: taffy::AvailableSpace::MaxContent,
                },
                &mut measure,
            )
            .expect("place_row: compute");
    }
}

impl Default for Headless {
    fn default() -> Self {
        Self::new()
    }
}

/// A widget built outside an `App`, for tests and for `node_tree::matches_fixture`.
pub struct BuiltWidget<Msg> {
    /// The widget's root retained node.
    pub node: Node,
    /// Its controller, already `build`-ed.
    pub controller: Box<dyn Controller<Msg>>,
}

/// Build one widget headlessly.
#[must_use]
pub fn build_widget<Msg: Clone + 'static>(kind: Kind, props: &Props) -> BuiltWidget<Msg> {
    let mut hx = Headless::new();
    let node = Node::with_classes(kind.css_name(), kind.base_classes());
    let controller = {
        let mut cx = hx.cx();
        build_controller::<Msg>(kind, &node, props, &mut cx)
    };
    BuiltWidget { node, controller }
}
