use icedtea_contract::Rectangle;

pub const TITLE_BAR_HEIGHT: i32 = 28;
pub const BUTTON_WIDTH: i32 = 40;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DecorationAction { Minimize, Maximize, Close, Move, None }

pub fn title_bar_rect(geometry: Rectangle) -> Rectangle {
    Rectangle { x: geometry.x, y: geometry.y, width: geometry.width, height: TITLE_BAR_HEIGHT }
}

pub fn button_rects(geometry: Rectangle) -> [Rectangle; 3] {
    let right = geometry.x + geometry.width;
    let y = geometry.y;
    [
        Rectangle { x: right - 3 * BUTTON_WIDTH, y, width: BUTTON_WIDTH, height: TITLE_BAR_HEIGHT },
        Rectangle { x: right - 2 * BUTTON_WIDTH, y, width: BUTTON_WIDTH, height: TITLE_BAR_HEIGHT },
        Rectangle { x: right - BUTTON_WIDTH, y, width: BUTTON_WIDTH, height: TITLE_BAR_HEIGHT },
    ]
}

pub fn hit_test(geometry: Rectangle, local: (i32, i32)) -> DecorationAction {
    let bar = title_bar_rect(geometry);
    if !bar.contains(local.0, local.1) {
        return DecorationAction::None;
    }
    for (i, r) in button_rects(geometry).iter().enumerate() {
        if r.contains(local.0, local.1) {
            return match i { 0 => DecorationAction::Minimize, 1 => DecorationAction::Maximize, _ => DecorationAction::Close };
        }
    }
    DecorationAction::Move
}

pub fn is_csd(app_id: &str, requested: Option<bool>) -> bool {
    // Explicit request wins; otherwise assume GTK-style apps use CSD.
    requested.unwrap_or(app_id.starts_with("org.gtk") || app_id.contains("gtk4"))
}

#[cfg(test)]
mod tests {
    use super::*;

    const GEO: Rectangle = Rectangle { x: 50, y: 50, width: 600, height: 400 };

    #[test]
    fn bar_is_top_strip() {
        let bar = title_bar_rect(GEO);
        assert_eq!(bar, Rectangle { x: 50, y: 50, width: 600, height: TITLE_BAR_HEIGHT });
    }

    #[test]
    fn close_button_is_rightmost() {
        let rects = button_rects(GEO);
        assert_eq!(rects[2].x, 50 + 600 - BUTTON_WIDTH);
    }

    #[test]
    fn buttons_and_move_areas() {
        let rects = button_rects(GEO);
        let inside_close = (rects[2].x + 1, rects[2].y + 1);
        assert_eq!(hit_test(GEO, inside_close), DecorationAction::Close);
        assert_eq!(hit_test(GEO, (100, 55)), DecorationAction::Move);
    }

    #[test]
    fn below_bar_is_none() {
        assert_eq!(hit_test(GEO, (100, 200)), DecorationAction::None);
    }

    #[test]
    fn csd_negotiation() {
        assert!(is_csd("org.gtk.MyApp", None));
        assert!(!is_csd("org.example.C", Some(false)));
        assert!(is_csd("anything", Some(true)));
    }
}
