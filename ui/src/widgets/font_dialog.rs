//! `GtkFontDialogButton` and `GtkFontDialog` — `Kind::FontDialogButton` (node
//! `fontbutton`) and `Kind::FontDialog` (node `window`, class `.dialog`).
//!
//! ```text
//! fontbutton
//! ╰── button.font
//!     ╰── [content]
//! ```
//!
//! ```text
//! window.dialog
//! ╰── fontchooser
//! ```
//!
//! Like `GtkColorDialog`, `GtkFontDialog` is a `GObject` launching a
//! deprecated chooser; icedtea builds the `fontchooser` body itself, listing
//! the families the M2 `FontDatabase` can actually match.

use std::rc::Rc;

use crate::css::node::Node;
use crate::css::value::{FontFamily, FontStyle, GenericFamily};
use crate::layout::{BoxDirection, Container, Rect};
use crate::text::FontQuery;
use crate::view::controller::{Controller, Event, EventCx};
use crate::view::{BuildCx, EventKind, Handler, Kind, Prop, PropName, Props, View};
use crate::widgets::{
    FontLevel, PointerState, WidgetEnum, local_rect, set_container, set_text, shift_event,
};

/// A `GtkFontDialogButton` showing `desc` (a Pango-style description).
#[must_use]
pub fn font_dialog_button<Msg: Clone + 'static>(desc: &str) -> View<Msg> {
    View::new(Kind::FontDialogButton).prop(PropName::Text, Prop::Str(Rc::from(desc)))
}

/// A `GtkFontDialog` body opened on `desc`.
#[must_use]
pub fn font_dialog<Msg: Clone + 'static>(desc: &str) -> View<Msg> {
    View::new(Kind::FontDialog).prop(PropName::Text, Prop::Str(Rc::from(desc)))
}

/// `GtkFontDialogButton`'s own setters.
pub trait FontDialogButtonExt<Msg>: Sized {
    /// `GtkFontDialogButton:use-font`.
    fn use_font(self, on: bool) -> Self;
    /// `GtkFontDialogButton:use-size`.
    fn use_size(self, on: bool) -> Self;
    /// `GtkFontDialogButton:level`.
    fn level(self, level: FontLevel) -> Self;
    // `GtkFontDialogButton:font-desc`'s change notification is the inherent
    // `View::on_change`, which shadows any same-named trait method at every
    // call site anyway.
}

impl<Msg: Clone + 'static> FontDialogButtonExt<Msg> for View<Msg> {
    fn use_font(self, on: bool) -> Self {
        self.prop(PropName::Markup, Prop::Bool(on))
    }
    fn use_size(self, on: bool) -> Self {
        self.prop(PropName::ShowText, Prop::Bool(on))
    }
    fn level(self, level: FontLevel) -> Self {
        self.prop(PropName::SelectionMode, level.to_prop())
    }
}

/// `GtkFontDialog`'s own setters.
pub trait FontDialogExt<Msg>: Sized {
    /// `GtkFontDialog:title`.
    fn title(self, text: &str) -> Self;
    /// `GtkFontDialog:modal`.
    fn modal(self, on: bool) -> Self;
    /// `GtkFontDialog:language`.
    fn language(self, lang: &str) -> Self;
    /// The dialog's response index.
    fn on_response(self, f: impl Fn(usize) -> Msg + 'static) -> Self;
    // The chosen font description arrives through the inherent
    // `View::on_change` -- see [`FontDialogButtonExt`].
}

impl<Msg: Clone + 'static> FontDialogExt<Msg> for View<Msg> {
    fn title(self, text: &str) -> Self {
        self.prop(PropName::Title, Prop::Str(Rc::from(text)))
    }
    fn modal(self, on: bool) -> Self {
        self.prop(PropName::Modal, Prop::Bool(on))
    }
    fn language(self, lang: &str) -> Self {
        self.prop(PropName::Detail, Prop::Str(Rc::from(lang)))
    }
    fn on_response(self, f: impl Fn(usize) -> Msg + 'static) -> Self {
        self.on(EventKind::Response, Handler::Index(Rc::new(f)))
    }
}

/// `Kind::FontDialogButton`'s controller.
pub struct FontDialogButtonC {
    /// The font description shown.
    pub desc: Rc<str>,
    /// Whether the dialog is showing.
    pub dialog_open: bool,
    /// The `button.font` subnode.
    pub button: Node,
    /// The `content` subnode carrying the description text.
    pub label: Node,
    /// A detached, never-attached sink: see
    /// `crate::widgets::color_dialog::ColorDialogButtonC::sink`.
    pub sink: Node,
    pointer: PointerState,
}

impl<Msg: Clone + 'static> Controller<Msg> for FontDialogButtonC {
    fn kind(&self) -> Kind {
        Kind::FontDialogButton
    }

    fn build(node: &Node, props: &Props, _cx: &mut BuildCx<'_>) -> Self {
        let button = Node::with_classes("button", &["font"]);
        node.append_child(&button);
        let label = Node::new("content");
        button.append_child(&label);
        FontDialogButtonC {
            desc: props
                .str(PropName::Text)
                .map_or_else(|| Rc::from(""), Rc::from),
            dialog_open: false,
            button,
            label,
            sink: Node::new("sink"),
            pointer: PointerState::default(),
        }
    }

    fn set_prop(&mut self, _node: &Node, name: PropName, value: &Prop, _cx: &mut BuildCx<'_>) {
        if let (PropName::Text, Prop::Str(desc)) = (name, value) {
            self.desc = Rc::clone(desc);
        }
    }

    fn on_event(&mut self, ev: &Event, cx: &mut EventCx<'_, Msg>) -> Vec<Msg> {
        let Some(rect) = local_rect(cx.tree, cx.node, &self.button) else {
            return Vec::new();
        };
        let shifted = shift_event(ev, rect);
        if self.pointer.observe(
            &self.button,
            &shifted,
            Some(Rect::new(0.0, 0.0, rect.width, rect.height)),
        ) {
            self.dialog_open = !self.dialog_open;
            cx.handled = true;
        }
        Vec::new()
    }
}

/// `Kind::FontDialog`'s controller — the chooser body icedtea builds itself.
#[non_exhaustive]
pub struct FontDialogC {
    /// Families the font database offered.
    pub families: Vec<Rc<str>>,
    /// The selected family, if any.
    pub selected: Option<usize>,
    /// The chosen size in px.
    pub size: f32,
    /// The preview string.
    pub preview: String,
    /// The `fontchooser` subnode; P6 replaces it with a real `ListViewC`.
    pub list: Node,
    /// One `row` subnode per family, in the same order as `families`.
    pub rows: Vec<Node>,
    /// A detached, never-attached sink: see
    /// `crate::widgets::color_dialog::ColorDialogButtonC::sink`.
    pub sink: Node,
}

impl FontDialogC {
    /// The families this database can actually match, in a stable order.
    ///
    /// `FontDatabase` has no "list every installed family" call (fontconfig's
    /// `FcFontList` is not exposed, and `probe_only` has only a fixed file
    /// list), so — exactly as the module doc says — the chooser lists the
    /// families the database *can match*: each CSS generic is resolved through
    /// [`FontDatabase::match_face`](crate::text::FontDatabase::match_face) and
    /// the concrete family it lands on is recorded once. With real fontconfig
    /// that is a handful of distinct faces; with the stripped-container probe
    /// it is whatever `FONT_CANDIDATES` files exist. Either way the list has
    /// real, measurable rows rather than the old single desc-derived entry.
    fn matchable_families(cx: &mut BuildCx<'_>) -> Vec<Rc<str>> {
        const GENERICS: &[GenericFamily] = &[
            GenericFamily::SansSerif,
            GenericFamily::Serif,
            GenericFamily::Monospace,
            GenericFamily::SystemUi,
            GenericFamily::Cursive,
            GenericFamily::Fantasy,
        ];
        let mut seen: Vec<Rc<str>> = Vec::new();
        for generic in GENERICS {
            let families = [FontFamily::Generic(*generic)];
            let query = FontQuery {
                families: &families,
                weight: 400.0,
                style: FontStyle::Normal,
                stretch: 100.0,
                size_px: 11.0,
            };
            if let Some(face) = cx.fonts.match_face(&query) {
                let name: Rc<str> = Rc::from(face.family.as_str());
                if !seen.iter().any(|f| f.as_ref() == name.as_ref()) {
                    seen.push(name);
                }
            }
        }
        seen
    }

    /// Resolve the row family list and the selected index for `family` — the
    /// desc's own family, already split off its size.
    ///
    /// The body is [`matchable_families`](Self::matchable_families). A
    /// non-empty `family` that is already one of them selects that existing
    /// row (no duplicate); a non-empty `family` that is *not* matched leads
    /// the list at index 0 and is the selected row; an empty `family` selects
    /// nothing. Shared by `build` and `set_prop` so the two can't diverge.
    fn resolve_families(family: &str, cx: &mut BuildCx<'_>) -> (Vec<Rc<str>>, Option<usize>) {
        let mut families = Self::matchable_families(cx);
        let selected = if family.is_empty() {
            None
        } else {
            match families.iter().position(|f| f.as_ref() == family) {
                Some(i) => Some(i),
                None => {
                    families.insert(0, Rc::from(family));
                    Some(0)
                }
            }
        };
        (families, selected)
    }

    /// Fill `list` with one `row` per family, each carrying the family name so
    /// [`measure_row`](crate::widgets::measure_row)/`paint_row` give it a real
    /// height and glyphs. `:selected` marks the chosen one. Detaches whatever
    /// rows were there before, so `set_prop` can refresh the body.
    fn rebuild_rows(&mut self) {
        for old in self.rows.drain(..) {
            old.detach();
        }
        for (i, family) in self.families.iter().enumerate() {
            let row = Node::with_classes("row", &["activatable"]);
            set_text(&row, family);
            row.set_state(
                crate::css::node::PseudoStates::SELECTED,
                self.selected == Some(i),
            );
            self.list.append_child(&row);
            self.rows.push(row);
        }
    }
}

impl<Msg: Clone + 'static> Controller<Msg> for FontDialogC {
    fn kind(&self) -> Kind {
        Kind::FontDialog
    }

    fn build(node: &Node, props: &Props, cx: &mut BuildCx<'_>) -> Self {
        node.add_class("dialog");
        let list = Node::new("fontchooser");
        // A vertical stack, so each family is its own full-width row.
        set_container(
            &list,
            Container::Box {
                direction: BoxDirection::Column,
            },
        );
        node.append_child(&list);
        // The description is `"<family> <size>"`, Pango's own shorthand.
        let desc = props.str(PropName::Text).unwrap_or("");
        let (family, size) = desc.rsplit_once(' ').unwrap_or((desc, "11"));
        // The body is the families the database can match; the desc's own
        // family leads the list (and is the selected row) when it is not
        // already one of them.
        let (families, selected) = Self::resolve_families(family, cx);
        let mut this = FontDialogC {
            families,
            selected,
            size: size
                .parse::<f32>()
                .ok()
                .filter(|s| s.is_finite() && *s > 0.0)
                .unwrap_or(11.0),
            preview: "The quick brown fox".to_owned(),
            list,
            rows: Vec::new(),
            sink: Node::new("sink"),
        };
        this.rebuild_rows();
        this
    }

    fn set_prop(&mut self, _node: &Node, name: PropName, value: &Prop, cx: &mut BuildCx<'_>) {
        if let (PropName::Text, Prop::Str(desc)) = (name, value) {
            let (family, size) = desc.rsplit_once(' ').unwrap_or((desc.as_ref(), "11"));
            let (families, selected) = Self::resolve_families(family, cx);
            self.selected = selected;
            self.families = families;
            self.size = size
                .parse::<f32>()
                .ok()
                .filter(|s| s.is_finite() && *s > 0.0)
                .unwrap_or(11.0);
            self.rebuild_rows();
        }
    }

    fn on_event(&mut self, _ev: &Event, _cx: &mut EventCx<'_, Msg>) -> Vec<Msg> {
        Vec::new()
    }
}

#[cfg(test)]
mod tests {
    use std::rc::Rc;

    use super::FontDialogC;
    use crate::css::node::{Node, PseudoStates};
    use crate::view::{Controller, Prop, PropName, Props};
    use crate::widgets::Headless;
    use crate::widgets::state::text_of;

    /// Build a `FontDialogC` on `desc`, headlessly. Returns the node (which the
    /// controller's rows hang under) alongside the concrete controller so tests
    /// can inspect `families`/`selected`/`rows` directly.
    fn build_dialog(desc: &str, hx: &mut Headless) -> (Node, FontDialogC) {
        let node = Node::new("window");
        let mut props = Props::default();
        props.set(PropName::Text, Prop::Str(Rc::from(desc)));
        let c = {
            let mut cx = hx.cx();
            <FontDialogC as Controller<()>>::build(&node, &props, &mut cx)
        };
        (node, c)
    }

    /// The families the headless database can match, computed the same way the
    /// controller does, so a test can pick a family that IS matched.
    fn matchable(hx: &mut Headless) -> Vec<Rc<str>> {
        let mut cx = hx.cx();
        FontDialogC::matchable_families(&mut cx)
    }

    /// Indices of the rows currently carrying `:selected`.
    fn selected_row_indices(c: &FontDialogC) -> Vec<usize> {
        c.rows
            .iter()
            .enumerate()
            .filter(|(_, r)| r.states().contains(PseudoStates::SELECTED))
            .map(|(i, _)| i)
            .collect()
    }

    #[test]
    fn one_row_is_built_per_resolved_family_with_matching_text() {
        // `rebuild_rows` must mint exactly one `row` per resolved family, in
        // order, each carrying that family's name -- not the old single
        // desc-derived entry. Mutation check: dropping a `families` entry or
        // mis-setting a row's text breaks the count/text equality below.
        let mut hx = Headless::new();
        let (_node, c) = build_dialog("Sans 12", &mut hx);

        assert_eq!(
            c.rows.len(),
            c.families.len(),
            "one row per resolved family"
        );
        for (row, family) in c.rows.iter().zip(&c.families) {
            assert_eq!(
                text_of(row).as_ref(),
                family.as_ref(),
                "each row carries its family's name"
            );
        }
    }

    #[test]
    fn an_unmatched_desc_family_leads_the_list_and_is_selected() {
        // A desc family the database cannot match must be prepended at index 0
        // and be the selected row -- so the chooser always shows what the desc
        // asked for even when it is not one of the matchable generics.
        let mut hx = Headless::new();
        let matched = matchable(&mut hx);
        const UNMATCHED: &str = "Nonexistent Zzz Family";
        assert!(
            !matched.iter().any(|f| f.as_ref() == UNMATCHED),
            "the fixture family must genuinely be unmatched"
        );

        let (_node, c) = build_dialog(&format!("{UNMATCHED} 12"), &mut hx);

        assert_eq!(c.families.len(), matched.len() + 1, "one row was added");
        assert_eq!(
            c.families[0].as_ref(),
            UNMATCHED,
            "the desc family leads the list"
        );
        assert_eq!(c.selected, Some(0));
        assert_eq!(
            selected_row_indices(&c),
            vec![0],
            "only the inserted row is `:selected`"
        );
        assert_eq!(text_of(&c.rows[0]).as_ref(), UNMATCHED);
    }

    #[test]
    fn a_matched_desc_family_inserts_no_duplicate_and_selects_the_existing_row() {
        // A desc family that IS already matchable must select that existing
        // row rather than prepend a second copy of it.
        let mut hx = Headless::new();
        let matched = matchable(&mut hx);
        let Some(fam) = matched.first().cloned() else {
            // No fonts at all (a stripped, fontconfig-free container): the
            // matched path cannot be exercised. The unmatched path is covered
            // above; nothing to assert here.
            return;
        };
        let want = matched.iter().position(|f| f.as_ref() == fam.as_ref());

        let (_node, c) = build_dialog(&format!("{fam} 12"), &mut hx);

        assert_eq!(
            c.families.len(),
            matched.len(),
            "no duplicate row was added"
        );
        assert_eq!(c.families, matched, "the matchable list is unchanged");
        assert_eq!(c.selected, want, "the existing row is selected");
        assert_eq!(
            selected_row_indices(&c),
            want.into_iter().collect::<Vec<_>>(),
            "exactly the matched row is `:selected`"
        );
    }

    #[test]
    fn a_text_update_rebuilds_rows_and_moves_the_selection() {
        // `set_prop(Text)` must rebuild the body and move `:selected`: from the
        // inserted unmatched row to a now-matched family, dropping the old
        // inserted row entirely.
        let mut hx = Headless::new();
        let matched = matchable(&mut hx);
        let Some(fam) = matched.first().cloned() else {
            return;
        };

        let (node, mut c) = build_dialog("Nonexistent Zzz Family 12", &mut hx);
        assert_eq!(c.selected, Some(0), "starts on the inserted unmatched row");
        assert_eq!(c.families.len(), matched.len() + 1);

        {
            let mut cx = hx.cx();
            <FontDialogC as Controller<()>>::set_prop(
                &mut c,
                &node,
                PropName::Text,
                &Prop::Str(Rc::from(format!("{fam} 24").as_str())),
                &mut cx,
            );
        }

        let want = matched.iter().position(|f| f.as_ref() == fam.as_ref());
        assert_eq!(
            c.families.len(),
            matched.len(),
            "the inserted unmatched family is gone; rows were rebuilt"
        );
        assert_eq!(
            c.rows.len(),
            c.families.len(),
            "rows track the new families"
        );
        assert_eq!(c.selected, want, "selection moved to the matched family");
        assert_eq!(
            selected_row_indices(&c),
            want.into_iter().collect::<Vec<_>>()
        );
        assert!(
            !c.families
                .iter()
                .any(|f| f.as_ref() == "Nonexistent Zzz Family"),
            "the previous unmatched family no longer appears"
        );
        assert!((c.size - 24.0).abs() < f32::EPSILON, "the new size parsed");
    }
}
