//! Text-decoration values.

use cssparser::Parser;

bitflags::bitflags! {
    /// `text-decoration-line`.
    #[derive(Copy, Clone, Debug, PartialEq, Eq, Default)]
    pub struct TextDecorationLines: u8 {
        /// `underline`.
        const UNDERLINE = 1;
        /// `overline`.
        const OVERLINE = 2;
        /// `line-through`.
        const LINE_THROUGH = 4;
        /// `blink` (parsed; never animated).
        const BLINK = 8;
    }
}

/// `none | [underline || overline || line-through || blink]`.
pub fn parse_decoration_lines(input: &mut Parser<'_, '_>) -> Result<TextDecorationLines, ()> {
    const LINES: &[(&str, TextDecorationLines)] = &[
        ("underline", TextDecorationLines::UNDERLINE),
        ("overline", TextDecorationLines::OVERLINE),
        ("line-through", TextDecorationLines::LINE_THROUGH),
        ("blink", TextDecorationLines::BLINK),
    ];
    let mut lines = TextDecorationLines::empty();
    let mut saw_none = false;
    let mut count = 0_u32;
    loop {
        let state = input.state();
        let name = match input.expect_ident() {
            Ok(name) => name.as_ref().to_string(),
            Err(_) => {
                input.reset(&state);
                break;
            }
        };
        if name.eq_ignore_ascii_case("none") {
            if count > 0 {
                return Err(());
            }
            saw_none = true;
            count += 1;
            continue;
        }
        if saw_none {
            return Err(());
        }
        let flag = LINES
            .iter()
            .find(|(spelling, _)| name.eq_ignore_ascii_case(spelling))
            .map(|(_, flag)| *flag)
            .ok_or(())?;
        if lines.contains(flag) {
            return Err(());
        }
        lines |= flag;
        count += 1;
    }
    if count == 0 { Err(()) } else { Ok(lines) }
}

#[cfg(test)]
mod tests {
    use super::{TextDecorationLines, parse_decoration_lines};
    use crate::css::value::parse_entirely_with;

    #[test]
    fn decoration_lines_combine_in_any_order() {
        let lines = |text: &str| parse_entirely_with(text, parse_decoration_lines).ok();
        assert_eq!(lines("none"), Some(TextDecorationLines::empty()));
        assert_eq!(lines("underline"), Some(TextDecorationLines::UNDERLINE));
        assert_eq!(
            lines("line-through UNDERLINE"),
            Some(TextDecorationLines::UNDERLINE | TextDecorationLines::LINE_THROUGH)
        );
        assert_eq!(
            lines("underline overline line-through blink"),
            Some(TextDecorationLines::all())
        );
        // A repeated keyword is invalid, and `none` cannot be combined.
        assert_eq!(lines("underline underline"), None);
        assert_eq!(lines("none underline"), None);
    }

    #[test]
    fn decoration_line_parsing_never_panics() {
        for input in crate::css::value::FUZZ_INPUTS {
            let _ = parse_entirely_with(input, parse_decoration_lines);
        }
    }
}
