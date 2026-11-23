use serde::Serialize;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Side {
    Left,
    Right,
}

// The SVG uses the same logical dimensions in src/desktop/railGeometry.ts.
pub const RAIL_WIDTH: i32 = 64;
pub const INDICATOR_DIAMETER: i32 = 76;
pub const INDICATOR_STEP: i32 = INDICATOR_DIAMETER + 12;
pub const RAIL_HEIGHT: i32 = INDICATOR_STEP + INDICATOR_DIAMETER;
pub const COLLAPSED_WIDTH: u8 = (INDICATOR_DIAMETER / 2) as u8;
pub const SUMMARY_WIDTH: i32 = 274;
pub const SUMMARY_HEIGHT: i32 = 192;
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
    pub summary: Rect,
    side: Side,
    scale: f64,
}

impl Layout {
    pub fn indicator_ellipses(&self, widths: [u8; 2]) -> [Rect; 2] {
        let edge = match self.side {
            Side::Left => 0,
            Side::Right => self.rail.width,
        };

        std::array::from_fn(|index| {
            let extent = pixels(i32::from(widths[index]), self.scale);
            let top = index as i32 * INDICATOR_STEP;
            Rect {
                x: edge - extent,
                y: pixels(top, self.scale),
                width: 2 * extent,
                height: pixels(top + INDICATOR_DIAMETER, self.scale) - pixels(top, self.scale),
            }
        })
    }
}

fn pixels(logical: i32, scale: f64) -> i32 {
    (f64::from(logical) * scale).round() as i32
}

// All returned coordinates and dimensions are physical pixels.
pub fn layout(work: Rect, scale: f64, side: Side, anchor: i32) -> Layout {
    let rail_width = pixels(RAIL_WIDTH, scale).min(work.width);
    let rail_height = pixels(RAIL_HEIGHT, scale).min(work.height);
    let right = work.x + work.width;
    let rail = Rect {
        x: match side {
            Side::Left => work.x,
            Side::Right => right - rail_width,
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
    Layout {
        rail,
        summary,
        side,
        scale,
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
        let result = layout(WORK, 1.0, Side::Right, 38);
        assert_eq!(
            result.rail,
            Rect {
                x: 1856,
                y: 438,
                width: 64,
                height: 164
            }
        );
        assert_eq!(
            result.summary,
            Rect {
                x: 1574,
                y: 380,
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
        let result = layout(work, 1.25, Side::Left, 126);
        assert_eq!(
            result.rail,
            Rect {
                x: -1600,
                y: 445,
                width: 80,
                height: 205
            }
        );
        assert_eq!(
            result.summary,
            Rect {
                x: -1510,
                y: 483,
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
        let result = layout(work, 1.0, Side::Right, 126);
        assert_eq!(result.rail.y, 40);
        assert_eq!(
            result.summary,
            Rect {
                x: 0,
                y: 40,
                width: 168,
                height: 120
            }
        );
        let result = layout(Rect { width: 64, ..work }, 1.0, Side::Left, 38);
        assert_eq!(result.summary.width, 0);
    }

    #[test]
    fn indicators_remain_at_the_edge_with_independent_widths_and_a_gap() {
        let widths = [COLLAPSED_WIDTH, RAIL_WIDTH as u8];
        let right = layout(WORK, 1.0, Side::Right, 38).indicator_ellipses(widths);
        let left = layout(WORK, 1.0, Side::Left, 38).indicator_ellipses(widths);
        assert_eq!(right[0].x, 26);
        assert_eq!(left[0].x, -38);
        assert_eq!(right[0].width, 76);
        assert_eq!(right[1].x, 0);
        assert_eq!(left[1].x, -64);
        assert_eq!(left[1].width, 128);
        assert_eq!(right[1].y - right[0].height, 12);
    }
}
