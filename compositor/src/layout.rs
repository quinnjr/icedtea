use icedtea_contract::Rectangle;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SnapZone { Left, Right, Top, Bottom, TopLeft, TopRight, BottomLeft, BottomRight }

pub fn snap_zone_for_point(output: Rectangle, point: (i32, i32), threshold: i32) -> Option<SnapZone> {
    let (x, y) = point;
    let near_left = x <= output.x + threshold;
    let near_right = x >= output.x + output.width - threshold;
    let near_top = y <= output.y + threshold;
    let near_bottom = y >= output.y + output.height - threshold;

    let edge = |h: bool, v: bool| match (h, v) {
        (true, true) => None,
        (true, false) => Some(SnapZone::Left),
        (false, true) => Some(SnapZone::Right),
        (false, false) => None,
    };
    let corner = |h: bool, v: bool| match (h, v) {
        (true, true) => Some(SnapZone::TopLeft),
        (true, false) => Some(SnapZone::TopRight),
        (false, true) => Some(SnapZone::BottomLeft),
        (false, false) => Some(SnapZone::BottomRight),
    };

    // Corners take priority over edges.
    let in_corner = (near_left || near_right) && (near_top || near_bottom);
    if in_corner {
        return corner(near_top, near_left);
    }
    if near_left || near_right {
        return edge(near_left, near_right);
    }
    if near_top || near_bottom {
        return if near_top && !near_bottom { Some(SnapZone::Top) } else if near_bottom && !near_top { Some(SnapZone::Bottom) } else { None };
    }
    None
}

pub fn snapped_geometry(output: Rectangle, zone: SnapZone, gap: i32) -> Rectangle {
    let w = output.width / 2;
    let h = output.height / 2;
    let (left_x, top_y, right_x, bottom_y) = (output.x, output.y, output.x + w, output.y + h);
    match zone {
        SnapZone::Left => Rectangle { x: output.x + gap, y: output.y + gap, width: w - 2 * gap, height: output.height - 2 * gap },
        SnapZone::Right => Rectangle { x: right_x + gap, y: output.y + gap, width: w - 2 * gap, height: output.height - 2 * gap },
        SnapZone::Top => Rectangle { x: output.x + gap, y: output.y + gap, width: output.width - 2 * gap, height: h - 2 * gap },
        SnapZone::Bottom => Rectangle { x: output.x + gap, y: bottom_y + gap, width: output.width - 2 * gap, height: h - 2 * gap },
        SnapZone::TopLeft => Rectangle { x: left_x + gap, y: top_y + gap, width: w - 2 * gap, height: h - 2 * gap },
        SnapZone::TopRight => Rectangle { x: right_x + gap, y: top_y + gap, width: w - 2 * gap, height: h - 2 * gap },
        SnapZone::BottomLeft => Rectangle { x: left_x + gap, y: bottom_y + gap, width: w - 2 * gap, height: h - 2 * gap },
        SnapZone::BottomRight => Rectangle { x: right_x + gap, y: bottom_y + gap, width: w - 2 * gap, height: h - 2 * gap },
    }
}

/// The geometry a maximized window occupies: the output rect inset by
/// `gap` on every side (review finding I5 -- maximize had no geometry
/// concept at all before).
pub fn maximized_geometry(output: Rectangle, gap: i32) -> Rectangle {
    Rectangle {
        x: output.x + gap,
        y: output.y + gap,
        width: (output.width - 2 * gap).max(1),
        height: (output.height - 2 * gap).max(1),
    }
}

pub fn restored_geometry(original: Rectangle, _snapped: Rectangle) -> Rectangle {
    original
}

/// Currently-unused public API: no production call site exists yet (the
/// drag/snap-preview path in `state.rs` calls `snap_zone_for_point`
/// directly rather than through this boolean wrapper). Retained for the
/// shell/drag integration, where "is this point near a snap edge at all"
/// is a natural predicate independent of *which* zone it is.
pub fn is_edge_point(output: Rectangle, point: (i32, i32), threshold: i32) -> bool {
    snap_zone_for_point(output, point, threshold).is_some()
}

pub fn cascade_point(occupied: &[Rectangle], _size: (i32, i32), step: i32) -> (i32, i32) {
    let base = (occupied.len() as i32 * step, occupied.len() as i32 * step);
    (base.0, base.1)
}

/// `cascade_point`, wrapped so a new window always lands fully inside
/// `output` (review finding M7: the unbounded version put the 40th window at
/// (960, 960) -- off-screen on a 1080p output, with no title bar to grab).
pub fn cascade_point_in(occupied: &[Rectangle], size: (i32, i32), step: i32, output: Rectangle) -> (i32, i32) {
    let (x, y) = cascade_point(occupied, size, step);
    let span_x = (output.width - size.0).max(1);
    let span_y = (output.height - size.1).max(1);
    (output.x + x.rem_euclid(span_x), output.y + y.rem_euclid(span_y))
}

#[cfg(test)]
mod tests {
    use super::*;

    const OUTPUT: Rectangle = Rectangle { x: 0, y: 0, width: 1000, height: 800 };

    #[test]
    fn left_edge_snaps_left_half() {
        let g = snapped_geometry(OUTPUT, SnapZone::Left, 8);
        assert_eq!(g.x, 8);
        assert_eq!(g.width, 1000 / 2 - 16);
        assert_eq!(g.height, 800 - 16);
    }

    #[test]
    fn quadrant_geometry() {
        let g = snapped_geometry(OUTPUT, SnapZone::TopRight, 0);
        assert_eq!(g.x, 500);
        assert_eq!(g.y, 0);
        assert_eq!(g.width, 500);
        assert_eq!(g.height, 400);
    }

    #[test]
    fn corner_wins_over_edge() {
        assert_eq!(snap_zone_for_point(OUTPUT, (0, 0), 10), Some(SnapZone::TopLeft));
        assert_eq!(snap_zone_for_point(OUTPUT, (999, 799), 10), Some(SnapZone::BottomRight));
        assert_eq!(snap_zone_for_point(OUTPUT, (500, 0), 10), Some(SnapZone::Top));
    }

    #[test]
    fn all_four_corners() {
        let threshold = 10;
        // Top-left corner
        assert_eq!(snap_zone_for_point(OUTPUT, (0, 0), threshold), Some(SnapZone::TopLeft));
        // Top-right corner
        assert_eq!(snap_zone_for_point(OUTPUT, (999, 0), threshold), Some(SnapZone::TopRight));
        // Bottom-left corner
        assert_eq!(snap_zone_for_point(OUTPUT, (0, 799), threshold), Some(SnapZone::BottomLeft));
        // Bottom-right corner
        assert_eq!(snap_zone_for_point(OUTPUT, (999, 799), threshold), Some(SnapZone::BottomRight));
    }

    #[test]
    fn center_is_not_a_zone() {
        assert_eq!(snap_zone_for_point(OUTPUT, (500, 400), 10), None);
    }

    #[test]
    fn gap_keeps_window_inside_output() {
        for zone in [SnapZone::Left, SnapZone::Right, SnapZone::Top, SnapZone::Bottom] {
            let g = snapped_geometry(OUTPUT, zone, 8);
            assert!(g.x >= 0 && g.y >= 0);
            assert!(g.x + g.width <= OUTPUT.width);
            assert!(g.y + g.height <= OUTPUT.height);
        }
    }

    #[test]
    fn maximized_is_output_minus_gap() {
        let g = maximized_geometry(OUTPUT, 8);
        assert_eq!(g, Rectangle { x: 8, y: 8, width: 1000 - 16, height: 800 - 16 });
        assert_eq!(maximized_geometry(OUTPUT, 0), OUTPUT);
    }

    #[test]
    fn maximized_never_degenerates_on_a_tiny_output() {
        let tiny = Rectangle { x: 0, y: 0, width: 10, height: 10 };
        let g = maximized_geometry(tiny, 8);
        assert!(g.width >= 1 && g.height >= 1);
    }

    #[test]
    fn cascade_wraps_inside_the_output() {
        // M7: the 40th window used to open at (960, 960) -- off-screen on a
        // 1080p output. Positions now wrap so a window always lands where it
        // can be seen and grabbed.
        let occupied: Vec<Rectangle> = (0..40).map(|_| Rectangle { x: 0, y: 0, width: 1, height: 1 }).collect();
        let (x, y) = cascade_point_in(&occupied, (640, 400), 24, OUTPUT);
        assert!(x >= OUTPUT.x && x + 640 <= OUTPUT.x + OUTPUT.width, "x = {x}");
        assert!(y >= OUTPUT.y && y + 400 <= OUTPUT.y + OUTPUT.height, "y = {y}");
    }

    #[test]
    fn restore_returns_original() {
        let orig = Rectangle { x: 10, y: 10, width: 200, height: 100 };
        assert_eq!(restored_geometry(orig, snapped_geometry(OUTPUT, SnapZone::Left, 8)), orig);
    }

    #[test]
    fn cascade_steps_by_count() {
        let occupied = vec![Rectangle { x: 0, y: 0, width: 100, height: 100 }];
        assert_eq!(cascade_point(&occupied, (200, 100), 24), (24, 24));
        assert_eq!(cascade_point(&[], (200, 100), 24), (0, 0));
    }
}
