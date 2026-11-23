#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Side {
    Left,
    Right,
}

pub const RAIL_WIDTH: i32 = 96;
pub const RAIL_HEIGHT: i32 = 210;
// Matches the visual border radius of .rail in styles.css.
const RAIL_CORNER_RADIUS: i32 = 18;
pub const SUMMARY_WIDTH: i32 = 274;
pub const SUMMARY_HEIGHT: i32 = 192;
const SCREEN_MARGIN: i32 = 10;
const SUMMARY_GAP: i32 = 8;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Rect {
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
}

pub struct Layout {
    pub rail: Rect,
    pub rail_corner_diameter: i32,
    pub summary: Rect,
}

fn pixels(logical: i32, scale: f64) -> i32 {
    (f64::from(logical) * scale).round() as i32
}

// All returned coordinates and dimensions are physical pixels.
pub fn layout(work: Rect, scale: f64, side: Side, anchor: i32) -> Layout {
    let rail_width = pixels(RAIL_WIDTH, scale).min(work.width);
    let rail_height = pixels(RAIL_HEIGHT, scale).min(work.height);
    let margin = pixels(SCREEN_MARGIN, scale).min(work.width - rail_width);
    let right = work.x + work.width;
    let rail = Rect {
        x: match side {
            Side::Left => work.x + margin,
            Side::Right => right - margin - rail_width,
        },
        y: work.y + (work.height - rail_height) / 2,
        width: rail_width,
        height: rail_height,
    };

    let (inside_left, inside_right) = match side {
        Side::Left => (rail.x + rail.width, right),
        Side::Right => (work.x, rail.x),
    };
    let inside_width = inside_right - inside_left;
    let gap = pixels(SUMMARY_GAP, scale).min(inside_width);
    let width = pixels(SUMMARY_WIDTH, scale).min(inside_width - gap);
    let height = pixels(SUMMARY_HEIGHT, scale).min(work.height);
    let summary = Rect {
        x: match side {
            Side::Left => inside_left + gap,
            Side::Right => inside_right - gap - width,
        },
        y: (rail.y + pixels(anchor, scale) - height / 2)
            .clamp(work.y, work.y + work.height - height),
        width,
        height,
    };
    let rail_corner_diameter = pixels(2 * RAIL_CORNER_RADIUS, scale)
        .min(rail_width)
        .min(rail_height);
    Layout {
        rail,
        rail_corner_diameter,
        summary,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const WORK: Rect = Rect {
        x: 0,
        y: 0,
        width: 1920,
        height: 1040,
    };

    #[test]
    fn right_is_centered_in_work_area_and_summary_opens_to_the_left() {
        let result = layout(WORK, 1.0, Side::Right, 62);
        assert_eq!(result.rail_corner_diameter, 36);
        assert_eq!(
            result.rail,
            Rect {
                x: 1814,
                y: 415,
                width: 96,
                height: 210
            }
        );
        assert_eq!(
            result.summary,
            Rect {
                x: 1532,
                y: 381,
                width: 274,
                height: 192
            }
        );
    }

    #[test]
    fn left_respects_work_area_origin_and_fractional_dpi() {
        let work = Rect {
            x: -1600,
            y: 48,
            width: 1600,
            height: 1000,
        };
        let result = layout(work, 1.25, Side::Left, 145);
        assert_eq!(result.rail_corner_diameter, 45);
        assert_eq!(
            result.rail,
            Rect {
                x: -1587,
                y: 416,
                width: 120,
                height: 263
            }
        );
        assert_eq!(
            result.summary,
            Rect {
                x: -1457,
                y: 477,
                width: 343,
                height: 240
            }
        );
    }

    #[test]
    fn summary_shrinks_to_fit_and_does_not_overlap_rail() {
        let work = Rect {
            x: 0,
            y: 40,
            width: 240,
            height: 120,
        };
        let result = layout(work, 1.0, Side::Right, 145);
        assert_eq!(result.rail.y, 40);
        assert_eq!(
            result.summary,
            Rect {
                x: 0,
                y: 40,
                width: 126,
                height: 120
            }
        );
        let result = layout(Rect { width: 80, ..work }, 1.0, Side::Left, 62);
        assert_eq!(result.summary.width, 0);
    }
}
