use serde::Serialize;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Side {
    Left,
    Right,
}

// Logical pixels, shared with src/desktop/railGeometry.ts.
pub const INDICATOR_DIAMETER: i32 = 52;
pub const INDICATOR_RADIUS: i32 = INDICATOR_DIAMETER / 2;
/// Distance between the expanded circle and the screen edge.
pub const EXPANDED_INSET: i32 = 6;
/// How far the circle center travels inward when expanded.
pub const SLIDE: u8 = (INDICATOR_RADIUS + EXPANDED_INSET) as u8;
pub const INDICATOR_GAP: i32 = 14;
pub const INDICATOR_STEP: i32 = INDICATOR_DIAMETER + INDICATOR_GAP;
pub const RAIL_WIDTH: i32 = SLIDE as i32 + INDICATOR_RADIUS + 2;
pub const RAIL_HEIGHT: i32 = INDICATOR_STEP + INDICATOR_DIAMETER;
/// The Win32 region extends past the drawn circle, so its aliased edge falls on
/// transparent pixels and the visible edge stays anti-aliased.
const REGION_PAD: i32 = 1;
pub const SUMMARY_WIDTH: i32 = 296;
pub const SUMMARY_MIN_HEIGHT: i32 = 120;
pub const SUMMARY_MAX_HEIGHT: i32 = 360;
const SUMMARY_GAP: i32 = 8;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Rect {
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
}

impl Rect {
    pub fn contains(&self, x: i32, y: i32) -> bool {
        x >= self.x && x < self.x + self.width && y >= self.y && y < self.y + self.height
    }

    pub fn union(&self, other: &Rect) -> Rect {
        let x = self.x.min(other.x);
        let y = self.y.min(other.y);
        Rect {
            x,
            y,
            width: (self.x + self.width).max(other.x + other.width) - x,
            height: (self.y + self.height).max(other.y + other.height) - y,
        }
    }
}

pub struct Layout {
    pub rail: Rect,
    pub summary: Rect,
    side: Side,
    scale: f64,
}

pub fn anchor(index: usize) -> i32 {
    index as i32 * INDICATOR_STEP + INDICATOR_RADIUS
}

impl Layout {
    /// Circle bounds in window coordinates for each slide offset, padded for
    /// the region. Collapsed circles are centered on the screen edge.
    pub fn indicator_circles(&self, shifts: [u8; 2]) -> [Rect; 2] {
        std::array::from_fn(|index| {
            let shift = i32::from(shifts[index].min(SLIDE));
            let center_x = match self.side {
                Side::Left => shift,
                Side::Right => RAIL_WIDTH - shift,
            };
            let top = index as i32 * INDICATOR_STEP;
            let left = pixels(center_x - INDICATOR_RADIUS - REGION_PAD, self.scale);
            let right = pixels(center_x + INDICATOR_RADIUS + REGION_PAD, self.scale);
            let upper = pixels(top - REGION_PAD, self.scale);
            let lower = pixels(top + INDICATOR_DIAMETER + REGION_PAD, self.scale);
            Rect {
                x: left,
                y: upper,
                width: right - left,
                height: lower - upper,
            }
        })
    }

    /// Screen area that keeps a hovered indicator open: the expanded circle,
    /// the summary card and the gap between them.
    pub fn hover_zone(&self, index: usize, summary: Option<Rect>) -> Zone {
        let mut shifts = [0; 2];
        shifts[index] = SLIDE;
        let circle = self.indicator_circles(shifts)[index];
        let circle = Rect {
            x: self.rail.x + circle.x,
            y: self.rail.y + circle.y,
            ..circle
        };
        let mut rects = vec![circle];
        if let Some(summary) = summary.filter(|summary| summary.width > 0) {
            // The bridge spans the gap at the height of the circle.
            rects.push(Rect {
                y: circle.y,
                height: circle.height,
                ..circle.union(&summary)
            });
            rects.push(summary);
        }
        Zone(rects)
    }
}

pub struct Zone(Vec<Rect>);

impl Zone {
    pub fn contains(&self, x: i32, y: i32) -> bool {
        self.0.iter().any(|rect| rect.contains(x, y))
    }
}

fn pixels(logical: i32, scale: f64) -> i32 {
    (f64::from(logical) * scale).round() as i32
}

// All returned coordinates and dimensions are physical pixels. `anchor` and
// `summary_height` are logical.
pub fn layout(work: Rect, scale: f64, side: Side, anchor: i32, summary_height: i32) -> Layout {
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
    let height = pixels(
        summary_height.clamp(SUMMARY_MIN_HEIGHT, SUMMARY_MAX_HEIGHT),
        scale,
    )
    .min(work.height);
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
    fn dimensions_follow_the_circle() {
        assert_eq!(SLIDE, 32);
        assert_eq!(RAIL_WIDTH, 60);
        assert_eq!(RAIL_HEIGHT, 118);
    }

    #[test]
    fn right_is_centered_in_work_area_and_summary_opens_inward() {
        let result = layout(WORK, 1.0, Side::Right, anchor(0), 180);
        assert_eq!(
            result.rail,
            Rect {
                x: 1860,
                y: 461,
                width: 60,
                height: 118
            }
        );
        assert_eq!(
            result.summary,
            Rect {
                x: 1556,
                y: 397,
                width: 296,
                height: 180
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
        let result = layout(work, 1.25, Side::Left, anchor(1), 200);
        assert_eq!(
            result.rail,
            Rect {
                x: -1600,
                y: 474,
                width: 75,
                height: 148
            }
        );
        assert_eq!(
            result.summary,
            Rect {
                x: -1515,
                y: 464,
                width: 370,
                height: 250
            }
        );
    }

    #[test]
    fn summary_height_is_clamped_and_never_overlaps_the_rail() {
        assert_eq!(layout(WORK, 1.0, Side::Right, 0, 10).summary.height, 120);
        assert_eq!(layout(WORK, 1.0, Side::Right, 0, 900).summary.height, 360);
        let work = Rect {
            x: 0,
            y: 40,
            width: 240,
            height: 100,
        };
        let result = layout(work, 1.0, Side::Right, anchor(1), 180);
        assert_eq!(result.rail.y, 40);
        assert_eq!(
            result.summary,
            Rect {
                x: 0,
                y: 40,
                width: 172,
                height: 100
            }
        );
        let result = layout(Rect { width: 60, ..work }, 1.0, Side::Left, anchor(0), 180);
        assert_eq!(result.summary.width, 0);
    }

    #[test]
    fn collapsed_circles_are_centered_on_the_edge_and_mirrored() {
        let right = layout(WORK, 1.0, Side::Right, 0, 0).indicator_circles([0, SLIDE]);
        let left = layout(WORK, 1.0, Side::Left, 0, 0).indicator_circles([0, SLIDE]);
        // Collapsed: the center sits on the edge, so exactly half is inside.
        assert_eq!(
            right[0],
            Rect {
                x: 33,
                y: -1,
                width: 54,
                height: 54
            }
        );
        assert_eq!(
            left[0],
            Rect {
                x: -27,
                y: -1,
                width: 54,
                height: 54
            }
        );
        assert_eq!(right[0].x + right[0].width / 2, RAIL_WIDTH);
        assert_eq!(left[0].x + left[0].width / 2, 0);
        // Expanded: the whole circle is inside, 6 px from the edge.
        assert_eq!(
            right[1],
            Rect {
                x: 1,
                y: 65,
                width: 54,
                height: 54
            }
        );
        assert_eq!(
            left[1],
            Rect {
                x: 5,
                y: 65,
                width: 54,
                height: 54
            }
        );
        assert_eq!(
            RAIL_WIDTH - (right[1].x + right[1].width - 1),
            EXPANDED_INSET
        );
        assert_eq!(left[1].x + 1, EXPANDED_INSET);
    }

    #[test]
    fn circles_scale_with_dpi() {
        let circles = layout(WORK, 1.5, Side::Right, 0, 0).indicator_circles([0, 0]);
        assert_eq!(circles[0].width, 81);
        assert_eq!(circles[0].x + circles[0].width / 2, 90);
        assert_eq!(circles[1].y, pixels(INDICATOR_STEP - 1, 1.5));
    }

    #[test]
    fn hover_zone_bridges_the_circle_and_the_summary() {
        let result = layout(WORK, 1.0, Side::Right, anchor(0), 180);
        let zone = result.hover_zone(0, Some(result.summary));
        // Gap between the summary's right edge and the expanded circle.
        assert!(zone.contains(1855, result.rail.y + 26));
        assert!(zone.contains(1600, 400));
        assert!(!zone.contains(1919, result.rail.y + 100));
        assert!(!zone.contains(1855, 380));
        // The neighbor indicator is not part of the zone.
        assert!(!zone.contains(1900, result.rail.y + INDICATOR_STEP + 26));
        let alone = result.hover_zone(1, None);
        assert!(alone.contains(1900, result.rail.y + INDICATOR_STEP + 26));
        assert!(!alone.contains(1900, result.rail.y + 26));
    }
}
