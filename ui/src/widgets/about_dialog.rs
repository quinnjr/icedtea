//! `GtkAboutDialog` — `Kind::AboutDialog`, CSS node `window`, class
//! `.aboutdialog` (`Kind::base_classes`).
//!
//! ```text
//! window.aboutdialog
//! ```
//!
//! One `WindowC` and one `StackC` embedded directly on this controller's
//! own root node — the same composition
//! [`crate::widgets::popover_menu::PopoverMenuC`] uses for its embedded
//! `PopoverC`. [`about_dialog`] takes no child views (contract §5.7: its
//! credits are flat name lists, not real per-page content the way
//! [`crate::widgets::stack::stack`]'s own pages are), so — the identical
//! fact [`crate::widgets::stack::StackC`]'s own module doc explains for
//! `StackC::place` reading an empty `node.children()` at `build` time —
//! `pages` is always empty here: there is no per-page `View` for the
//! reconciler ever to attach. It stays a real embedded `StackC` (rather
//! than being dropped from the controller) because the credits-page
//! *machinery* — `visible_child`, transition progress — is exactly
//! `StackC`'s, and a future part giving `AboutDialog` real page content
//! only has to attach children for `StackC::place` to pick up, the same way
//! it already does for a real `stack(pages)` view.
//!
//! `website`/`license_type` are the only two props this controller reads
//! back out for its own logic (`website_of`'s payload, the licence page's
//! short name); everything else `about_dialog`'s fourteen setters
//! write is either display-only credits text no widget here paints yet (the
//! same trade `alert_dialog`'s title/detail labels make — see that
//! module's doc) or handled generically by `WindowC`'s/`StackC`'s own
//! `set_prop`.

use std::rc::Rc;

use crate::css::node::Node;
use crate::view::controller::{Controller, Event, EventCx};
use crate::view::{BuildCx, EventKind, Kind, Prop, PropName, Props, View};
use crate::widgets::prop_u16;
use crate::widgets::stack::StackC;
use crate::widgets::types::LicenseType;
use crate::widgets::window::WindowC;

/// A `GtkAboutDialog` naming `program_name`.
#[must_use]
pub fn about_dialog<Msg: Clone + 'static>(program_name: &str) -> View<Msg> {
    View::new(Kind::AboutDialog).prop(PropName::ProgramName, Prop::Str(Rc::from(program_name)))
}

/// [`about_dialog`]'s own setters, one line per `GtkAboutDialog` property
/// contract §5.7 names.
pub trait AboutDialogExt<Msg>: Sized {
    /// `GtkAboutDialog:version`.
    fn version(self, text: &str) -> Self;
    /// `GtkAboutDialog:comments`.
    fn comments(self, text: &str) -> Self;
    /// `GtkAboutDialog:copyright`.
    fn copyright(self, text: &str) -> Self;
    /// `GtkAboutDialog:license`, the application's own text.
    fn license(self, text: &str) -> Self;
    /// `GtkAboutDialog:license-type`, a well-known licence.
    fn license_type(self, ty: LicenseType) -> Self;
    /// `GtkAboutDialog:website`.
    fn website(self, uri: &str) -> Self;
    /// `GtkAboutDialog:website-label`.
    fn website_label(self, text: &str) -> Self;
    /// `GtkAboutDialog:authors`.
    fn authors(self, names: impl IntoIterator<Item = impl Into<Rc<str>>>) -> Self;
    /// `GtkAboutDialog:artists`.
    fn artists(self, names: impl IntoIterator<Item = impl Into<Rc<str>>>) -> Self;
    /// `GtkAboutDialog:documenters`.
    fn documenters(self, names: impl IntoIterator<Item = impl Into<Rc<str>>>) -> Self;
    /// `GtkAboutDialog:translator-credits`.
    fn translator_credits(self, text: &str) -> Self;
    /// `GtkAboutDialog:logo-icon-name`.
    fn logo_icon_name(self, name: &str) -> Self;
    /// `GtkAboutDialog:wrap-license`.
    fn wrap_license(self, on: bool) -> Self;
}

impl<Msg: Clone + 'static> AboutDialogExt<Msg> for View<Msg> {
    fn version(self, text: &str) -> Self {
        self.prop(PropName::Version, Prop::Str(Rc::from(text)))
    }
    fn comments(self, text: &str) -> Self {
        self.prop(PropName::Comments, Prop::Str(Rc::from(text)))
    }
    fn copyright(self, text: &str) -> Self {
        self.prop(PropName::Copyright, Prop::Str(Rc::from(text)))
    }
    fn license(self, text: &str) -> Self {
        self.prop(PropName::License, Prop::Str(Rc::from(text)))
    }
    fn license_type(self, ty: LicenseType) -> Self {
        self.prop(PropName::LicenseType, Prop::Enum(ty.to_u16()))
    }
    fn website(self, uri: &str) -> Self {
        self.prop(PropName::Website, Prop::Str(Rc::from(uri)))
    }
    fn website_label(self, text: &str) -> Self {
        self.prop(PropName::WebsiteLabel, Prop::Str(Rc::from(text)))
    }
    fn authors(self, names: impl IntoIterator<Item = impl Into<Rc<str>>>) -> Self {
        self.prop(
            PropName::Authors,
            Prop::Classes(names.into_iter().map(Into::into).collect()),
        )
    }
    fn artists(self, names: impl IntoIterator<Item = impl Into<Rc<str>>>) -> Self {
        self.prop(
            PropName::Artists,
            Prop::Classes(names.into_iter().map(Into::into).collect()),
        )
    }
    fn documenters(self, names: impl IntoIterator<Item = impl Into<Rc<str>>>) -> Self {
        self.prop(
            PropName::Documenters,
            Prop::Classes(names.into_iter().map(Into::into).collect()),
        )
    }
    fn translator_credits(self, text: &str) -> Self {
        self.prop(PropName::TranslatorCredits, Prop::Str(Rc::from(text)))
    }
    fn logo_icon_name(self, name: &str) -> Self {
        self.prop(PropName::LogoIconName, Prop::Str(Rc::from(name)))
    }
    fn wrap_license(self, on: bool) -> Self {
        self.prop(PropName::WrapLicense, Prop::Bool(on))
    }
}

/// `Kind::AboutDialog`'s controller.
pub struct AboutDialogC {
    /// The embedded `Window` chrome — see the module doc.
    pub window: WindowC,
    /// The embedded credits-page machinery — see the module doc; always
    /// empty (no page `View`s exist for this widget to attach).
    pub pages: StackC,
    /// `GtkAboutDialog:website`, for [`AboutDialogC::website_of`].
    website: Rc<str>,
    /// `GtkAboutDialog:license-type`, for
    /// [`AboutDialogC::license_short_name`].
    license_type: LicenseType,
}

impl AboutDialogC {
    /// GTK's own short name for a well-known licence
    /// (`gtk_license_to_string`, abbreviated the way the licence page's
    /// heading does) — `Custom`/`Unknown` name nothing, since the text (if
    /// any) is the application's own `PropName::License`.
    #[must_use]
    pub fn license_short_name(ty: LicenseType) -> &'static str {
        match ty {
            LicenseType::Unknown | LicenseType::Custom => "",
            LicenseType::Gpl20 => "GNU GPL v2.0",
            LicenseType::Gpl30 => "GNU GPL v3.0",
            LicenseType::Lgpl21 => "GNU LGPL v2.1",
            LicenseType::Lgpl30 => "GNU LGPL v3.0",
            LicenseType::Agpl30 => "GNU AGPL v3.0",
            LicenseType::Bsd => "BSD 2-Clause",
            LicenseType::MitX11 => "MIT/X11",
            LicenseType::Apache20 => "Apache 2.0",
            LicenseType::Mpl20 => "MPL 2.0",
        }
    }

    /// `GtkAboutDialog:website`, when non-empty — the payload
    /// `EventKind::ActivateLink` would carry once a future part builds this
    /// widget's real website link (the module doc: no page `View` exists
    /// yet to host one, so nothing in [`AboutDialogC::on_event`] fires it).
    /// Test hook, the shape this crate's other test hooks
    /// (`StackC::visible_of`) use.
    #[must_use]
    pub fn website_of<Msg: 'static>(c: &dyn Controller<Msg>) -> Option<Rc<str>> {
        let any: &dyn std::any::Any = c;
        any.downcast_ref::<Self>()
            .filter(|m| !m.website.is_empty())
            .map(|m| Rc::clone(&m.website))
    }
}

impl<Msg: Clone + 'static> Controller<Msg> for AboutDialogC {
    fn kind(&self) -> Kind {
        Kind::AboutDialog
    }

    fn build(node: &Node, props: &Props, cx: &mut BuildCx<'_>) -> Self {
        let window = <WindowC as Controller<Msg>>::build(node, props, cx);
        let pages = <StackC as Controller<Msg>>::build(node, props, cx);
        AboutDialogC {
            window,
            pages,
            website: props
                .str(PropName::Website)
                .map_or_else(|| Rc::from(""), Rc::from),
            license_type: match props.get(PropName::LicenseType) {
                Some(value) => LicenseType::from_u16(prop_u16(value, 0)),
                None => LicenseType::default(),
            },
        }
    }

    fn set_prop(&mut self, node: &Node, name: PropName, value: &Prop, cx: &mut BuildCx<'_>) {
        match (name, value) {
            (PropName::Website, Prop::Str(s)) => self.website = Rc::clone(s),
            (PropName::LicenseType, _) => {
                self.license_type =
                    LicenseType::from_u16(prop_u16(value, self.license_type as u16));
            }
            _ => {
                <StackC as Controller<Msg>>::set_prop(&mut self.pages, node, name, value, cx);
                <WindowC as Controller<Msg>>::set_prop(&mut self.window, node, name, value, cx);
            }
        }
    }

    fn on_event(&mut self, ev: &Event, cx: &mut EventCx<'_, Msg>) -> Vec<Msg> {
        let mut msgs = <WindowC as Controller<Msg>>::on_event(&mut self.window, ev, cx);
        if let Event::Key(key) = ev
            && key.pressed
        {
            use xkbcommon::xkb::keysyms;
            if u32::from(key.keysym) == keysyms::KEY_Escape {
                cx.handled = true;
                if let Some(m) = cx.handlers.fire_unit(EventKind::Close) {
                    msgs.push(m);
                }
            }
        }
        msgs
    }

    fn tick(&mut self, now: std::time::Duration, cx: &mut EventCx<'_, Msg>) -> Vec<Msg> {
        <StackC as Controller<Msg>>::tick(&mut self.pages, now, cx)
    }

    fn next_deadline(&self, now: std::time::Duration) -> Option<std::time::Duration> {
        <StackC as Controller<Msg>>::next_deadline(&self.pages, now)
    }

    fn reserved_total(&self, view_count: usize) -> usize {
        <WindowC as Controller<Msg>>::reserved_total(&self.window, view_count);
        <StackC as Controller<Msg>>::reserved_total(&self.pages, view_count);
        view_count
    }
}

#[cfg(test)]
mod tests {
    use crate::view::{Event, EventKind, Handler, Kind, Prop, PropName, Props};
    use crate::widgets::about_dialog::AboutDialogC;
    use crate::widgets::types::LicenseType;
    use crate::widgets::{Headless, build_widget, matches_fixture};

    fn props() -> Props {
        let mut p = Props::default();
        p.set(PropName::ProgramName, Prop::Str("Files".into()));
        p.set(PropName::Website, Prop::Str("https://example.org".into()));
        p.set(
            PropName::LicenseType,
            Prop::Enum(LicenseType::Gpl30.to_u16()),
        );
        p
    }

    #[test]
    fn the_window_is_window_aboutdialog() {
        // Mutation check: dropping `.aboutdialog` from `Kind::base_classes`
        // leaves this indistinguishable from a plain `Window` in CSS.
        let built = build_widget::<()>(Kind::AboutDialog, &props());
        matches_fixture(&built.node, "window.aboutdialog\n").expect("about_dialog fixture");
    }

    #[test]
    fn escape_closes_and_a_set_website_can_be_activated() {
        // Interaction test.
        let built = build_widget::<String>(Kind::AboutDialog, &props());
        let mut c = built.controller;
        let mut hx = Headless::new();
        let mut cx = hx.event_cx_with_handlers::<String>(&built.node, |h| {
            h.set(EventKind::Close, Handler::Unit("closed".to_string()));
        });
        assert_eq!(
            c.on_event(&Event::Key(Headless::key("Escape")), &mut cx),
            vec!["closed".to_string()]
        );
        assert_eq!(
            AboutDialogC::website_of::<String>(c.as_ref()),
            Some(std::rc::Rc::from("https://example.org"))
        );
    }

    #[test]
    fn unknown_license_types_fall_back_to_a_plain_short_name_not_panicking() {
        // Mutation check: an unmapped `LicenseType` variant must not panic
        // `license_short_name` -- the licence page still renders something.
        assert_eq!(AboutDialogC::license_short_name(LicenseType::Unknown), "");
        assert_eq!(AboutDialogC::license_short_name(LicenseType::Custom), "");
        assert_eq!(
            AboutDialogC::license_short_name(LicenseType::Gpl30),
            "GNU GPL v3.0"
        );
    }
}
