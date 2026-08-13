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

/// Which title-bar button, if any, `local` lands on -- `button_rects`' own
/// index (0 minimize, 1 maximize, 2 close), the same order `hit_test` maps
/// to `Minimize`/`Maximize`/`Close`. Pointer-motion hover tracking needs the
/// index itself (to pick which of `sync_ssd`'s three button colors to
/// shift), where `hit_test`'s `DecorationAction` is enough for a click but
/// throws the index away.
pub fn button_at(geometry: Rectangle, local: (i32, i32)) -> Option<usize> {
    let bar = title_bar_rect(geometry);
    if !bar.contains(local.0, local.1) {
        return None;
    }
    button_rects(geometry).iter().position(|r| r.contains(local.0, local.1))
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

/// Whether *we* draw a title bar over this window's frame. The single
/// definition of the predicate `render::draw_frame` and
/// `State::sync_window_to_space` both have to agree on (re-review finding
/// New-4): a CSD client draws its own decorations, and a fullscreen window
/// has none at all, so only the remaining case reserves a strip.
pub fn has_ssd(app_id: &str, requested: Option<bool>, fullscreen: bool) -> bool {
    !fullscreen && !is_csd(app_id, requested)
}

/// The client content rect inside a window's frame geometry.
///
/// Re-review finding New-4: the model's `geometry` describes the whole
/// *frame*, and for a server-side-decorated window we paint a
/// `TITLE_BAR_HEIGHT` strip across the top of it (`title_bar_rect`). The
/// client used to be configured at -- and mapped at -- the full frame rect,
/// so the strip landed on top of the buffer's first 28 rows and permanently
/// occluded whatever the client drew there. The fix is this inset: the
/// client is told it has `height - TITLE_BAR_HEIGHT` and is mapped
/// `TITLE_BAR_HEIGHT` lower, which is exactly the band `title_bar_rect`
/// occupies. `ssd == false` (CSD, or fullscreen) yields the frame unchanged
/// -- those windows own every pixel of their geometry.
///
/// Width and height are floored at 1 on *both* branches: xdg-shell has no
/// meaningful zero/negative size, and a frame shorter than the title bar
/// (only reachable from a degenerate model geometry) must not configure a
/// client with one. Callers can stage the result directly without their own
/// floor (re-review minor 5: a single owner for the floor, not one per
/// branch split across two files).
pub fn content_rect(geometry: Rectangle, ssd: bool) -> Rectangle {
    if !ssd {
        return Rectangle {
            width: geometry.width.max(1),
            height: geometry.height.max(1),
            ..geometry
        };
    }
    Rectangle {
        x: geometry.x,
        y: geometry.y + TITLE_BAR_HEIGHT,
        width: geometry.width.max(1),
        height: (geometry.height - TITLE_BAR_HEIGHT).max(1),
    }
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
    fn button_at_maps_to_button_rects_index() {
        let rects = button_rects(GEO);
        assert_eq!(button_at(GEO, (rects[0].x + 1, rects[0].y + 1)), Some(0));
        assert_eq!(button_at(GEO, (rects[1].x + 1, rects[1].y + 1)), Some(1));
        assert_eq!(button_at(GEO, (rects[2].x + 1, rects[2].y + 1)), Some(2));
        assert_eq!(button_at(GEO, (100, 55)), None, "move area is not a button");
        assert_eq!(button_at(GEO, (100, 200)), None, "below the bar is not a button");
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

    // --- New-4: the SSD content inset ---

    #[test]
    fn has_ssd_only_for_decorated_non_fullscreen_windows() {
        assert!(has_ssd("org.example.C", Some(false), false));
        assert!(!has_ssd("org.example.C", Some(false), true), "fullscreen has no strip");
        assert!(!has_ssd("org.gtk.MyApp", None, false), "CSD draws its own");
    }

    #[test]
    fn content_rect_insets_ssd_windows_by_exactly_the_title_bar() {
        let content = content_rect(GEO, true);
        assert_eq!(
            content,
            Rectangle {
                x: 50,
                y: 50 + TITLE_BAR_HEIGHT,
                width: 600,
                height: 400 - TITLE_BAR_HEIGHT
            }
        );
        // The freed band is precisely the strip the renderer paints, with
        // no overlap and no gap.
        let bar = title_bar_rect(GEO);
        assert_eq!(bar.y + bar.height, content.y);
        assert_eq!(content.y + content.height, GEO.y + GEO.height);
    }

    #[test]
    fn content_rect_is_the_frame_for_csd_and_fullscreen() {
        assert_eq!(content_rect(GEO, false), GEO);
    }

    #[test]
    fn content_rect_floors_a_degenerate_frame_at_one_pixel() {
        let tiny = Rectangle { x: 0, y: 0, width: 0, height: TITLE_BAR_HEIGHT - 1 };
        let content = content_rect(tiny, true);
        assert_eq!(content.width, 1);
        assert_eq!(content.height, 1);
    }
}
