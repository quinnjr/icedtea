//! Scaffolding gate for M2 Part 1. Deleted in the final Part 1 task.

#[test]
fn token_helpers_live_in_css_tokens() {
    assert_eq!(
        icedtea_ui::css::tokens::component_values("1px solid rgb(0 0 0)"),
        vec!["1px", "solid", "rgb(0 0 0)"]
    );
}

#[test]
fn bitflags_is_available_to_the_crate() {
    bitflags::bitflags! {
        #[derive(Copy, Clone, Debug, PartialEq, Eq)]
        struct Probe: u8 { const A = 1; const B = 2; }
    }
    assert_eq!((Probe::A | Probe::B).bits(), 3);
}
