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
        return corner(near_left, near_top);
    }
    if near_left || near_right {
        return edge(near_left, near_right);
    }
    if near_top || near_bottom {
        return if near_top { Some(SnapZone::Top) } else { Some(SnapZone::Bottom) };
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

pub fn restored_geometry(original: Rectangle, _snapped: Rectangle) -> Rectangle {
    original
}

pub fn is_edge_point(output: Rectangle, point: (i32, i32), threshold: i32) -> bool {
    snap_zone_for_point(output, point, threshold).is_some()
}

pub fn cascade_point(occupied: &[Rectangle], _size: (i32, i32), step: i32) -> (i32, i32) {
    let base = (occupied.len() as i32 * step, occupied.len() as i32 * step);
    (base.0, base.1)
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
