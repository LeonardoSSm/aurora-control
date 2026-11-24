use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
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
/// Most indicators the rail can hold: one per catalog provider.
pub const MAX_INDICATORS: usize = crate::usage::Provider::ALL.len();
/// Slide offset of each indicator slot, from the top. Unused slots stay 0.
pub type Shifts = [u8; MAX_INDICATORS];
#[cfg(test)]
pub const RAIL_HEIGHT: i32 = INDICATOR_STEP + INDICATOR_DIAMETER;
/// The Win32 region extends past the drawn circle, so its aliased edge falls on
/// transparent pixels and the visible edge stays anti-aliased.
const REGION_PAD: i32 = 1;
pub const SUMMARY_WIDTH: i32 = 296;
pub const SUMMARY_MIN_HEIGHT: i32 = 120;
pub const SUMMARY_MAX_HEIGHT: i32 = 360;
const SUMMARY_GAP: i32 = 8;

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
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
    /// Visible indicators, from the top (1 or 2).
    slots: usize,
}

pub fn anchor(index: usize) -> i32 {
    index as i32 * INDICATOR_STEP + INDICATOR_RADIUS
}

impl Layout {
    /// Circle bounds in window coordinates for each slide offset, padded for
    /// the region. Collapsed circles are centered on the screen edge.
    pub fn indicator_circles(&self, shifts: Shifts) -> [Rect; MAX_INDICATORS] {
        std::array::from_fn(|index| {
            let shift = i32::from(shifts[index].min(SLIDE));
            let center_x = match self.side {
                Side::Left => shift,
                Side::Right => RAIL_WIDTH - shift,
            };
            let top = index as i32 * INDICATOR_STEP;
            // The rail may be narrower than its logical width on a tiny work area:
            // the right side then anchors on the real window edge (audit I07).
            let narrowing = match self.side {
                Side::Left => 0,
                Side::Right => self.rail.width - pixels(RAIL_WIDTH, self.scale),
            };
            let left = narrowing + pixels(center_x - INDICATOR_RADIUS - REGION_PAD, self.scale);
            let right = narrowing + pixels(center_x + INDICATOR_RADIUS + REGION_PAD, self.scale);
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

    /// The circles of the visible indicators, for the window region.
    pub fn visible_circles(&self, shifts: Shifts) -> Vec<Rect> {
        self.indicator_circles(shifts)
            .into_iter()
            .take(self.slots)
            .collect()
    }

    /// The input shape of one indicator on screen, for the slide offsets of
    /// the last frame Windows accepted: the same ellipse as the window region,
    /// clipped to the rail window. Transparent corners are outside (audit I04).
    pub fn indicator_shape(&self, index: usize, shifts: Shifts) -> Shape {
        let circle = self.indicator_circles(shifts)[index];
        Shape::Ellipse {
            bounds: Rect {
                x: self.rail.x + circle.x,
                y: self.rail.y + circle.y,
                ..circle
            },
            clip: self.rail,
        }
    }

    /// Screen area that keeps a hovered indicator open: the indicator as
    /// drawn in the accepted frame, the summary card and the bridge between
    /// them at the height of the circle.
    pub fn hover_zone(&self, index: usize, shifts: Shifts, summary: Option<Rect>) -> Zone {
        let circle = self.indicator_shape(index, shifts);
        let mut shapes = vec![circle];
        if let (Some(summary), Shape::Ellipse { bounds, .. }) =
            (summary.filter(|summary| summary.width > 0), circle)
        {
            // The bridge spans from the circle center to the card.
            let center = Rect {
                x: bounds.x + bounds.width / 2,
                width: 1,
                ..bounds
            };
            shapes.push(Shape::Rect(Rect {
                y: bounds.y,
                height: bounds.height,
                ..center.union(&summary)
            }));
            shapes.push(Shape::Rect(summary));
        }
        Zone(shapes)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Shape {
    Rect(Rect),
    /// The ellipse inscribed in `bounds`, intersected with `clip`.
    Ellipse {
        bounds: Rect,
        clip: Rect,
    },
}

impl Shape {
    pub fn contains(&self, x: i32, y: i32) -> bool {
        match *self {
            Shape::Rect(rect) => rect.contains(x, y),
            Shape::Ellipse { bounds, clip } => {
                if !clip.contains(x, y) || !bounds.contains(x, y) {
                    return false;
                }
                // Pixel centers against the normalized ellipse equation.
                let rx = f64::from(bounds.width) / 2.0;
                let ry = f64::from(bounds.height) / 2.0;
                let dx = (f64::from(x - bounds.x) + 0.5 - rx) / rx;
                let dy = (f64::from(y - bounds.y) + 0.5 - ry) / ry;
                dx * dx + dy * dy <= 1.0
            }
        }
    }
}

pub struct Zone(Vec<Shape>);

impl Zone {
    pub fn contains(&self, x: i32, y: i32) -> bool {
        self.0.iter().any(|shape| shape.contains(x, y))
    }
}

fn pixels(logical: i32, scale: f64) -> i32 {
    (f64::from(logical) * scale).round() as i32
}

// All returned coordinates and dimensions are physical pixels. `anchor` and
// `summary_height` are logical.
/// Height of the rail for `slots` visible indicators, in logical pixels.
pub fn rail_height(slots: usize) -> i32 {
    slots.clamp(1, MAX_INDICATORS) as i32 * INDICATOR_STEP - INDICATOR_GAP
}

/// How many indicators fit, with their regular spacing, in a work area of
/// `work_height` physical pixels. At least one: a tiny work area shows the
/// first indicator, clipped to the area, rather than none. Indicators past
/// the capacity are not drawn (the first ones in catalog order are kept);
/// the window never extends past the work area. Four indicators need 250
/// logical pixels, so this only matters on very small or very scaled screens.
pub fn capacity(work_height: i32, scale: f64) -> usize {
    let scale = if scale.is_finite() && scale > 0.0 {
        scale
    } else {
        1.0
    };
    let logical = (f64::from(work_height.max(0)) / scale).floor() as i32;
    let fit = (logical + INDICATOR_GAP) / INDICATOR_STEP;
    usize::try_from(fit.max(1)).unwrap_or(1).min(MAX_INDICATORS)
}

/// [`layout_with`] for both indicators.
#[cfg(test)]
pub fn layout(work: Rect, scale: f64, side: Side, anchor: i32, summary_height: i32) -> Layout {
    layout_with(work, scale, side, 2, anchor, summary_height)
}

pub fn layout_with(
    work: Rect,
    scale: f64,
    side: Side,
    slots: usize,
    anchor: i32,
    summary_height: i32,
) -> Layout {
    // Zero visible indicators hide the rail before any layout is taken.
    let slots = slots
        .clamp(1, MAX_INDICATORS)
        .min(capacity(work.height, scale));
    let rail_width = pixels(RAIL_WIDTH, scale).min(work.width);
    let rail_height = pixels(rail_height(slots), scale).min(work.height);
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
        slots,
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
        // 100 px fit one indicator (capacity), centered: no clipped second circle.
        assert_eq!((result.rail.y, result.rail.height), (64, 52));
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
        let right = layout(WORK, 1.0, Side::Right, 0, 0).indicator_circles([0, SLIDE, 0, 0]);
        let left = layout(WORK, 1.0, Side::Left, 0, 0).indicator_circles([0, SLIDE, 0, 0]);
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
        let circles = layout(WORK, 1.5, Side::Right, 0, 0).indicator_circles([0; MAX_INDICATORS]);
        assert_eq!(circles[0].width, 81);
        assert_eq!(circles[0].x + circles[0].width / 2, 90);
        assert_eq!(circles[1].y, pixels(INDICATOR_STEP - 1, 1.5));
    }

    #[test]
    fn hover_zone_bridges_the_circle_and_the_summary() {
        let result = layout(WORK, 1.0, Side::Right, anchor(0), 180);
        let expanded = [SLIDE, 0, 0, 0];
        let zone = result.hover_zone(0, expanded, Some(result.summary));
        // Gap between the summary's right edge and the expanded circle.
        assert!(zone.contains(1855, result.rail.y + 26));
        assert!(zone.contains(1600, 400));
        assert!(!zone.contains(1919, result.rail.y + 100));
        assert!(!zone.contains(1855, 380));
        // The neighbor indicator is not part of the zone.
        assert!(!zone.contains(1900, result.rail.y + INDICATOR_STEP + 26));
        let alone = result.hover_zone(1, [0; MAX_INDICATORS], None);
        assert!(alone.contains(1900, result.rail.y + INDICATOR_STEP + 26));
        assert!(!alone.contains(1900, result.rail.y + 26));
    }

    #[test]
    fn i04_transparent_corners_and_the_unreached_final_frame_are_outside() {
        for scale in [1.0, 1.25, 1.5] {
            for side in [Side::Right, Side::Left] {
                let result = layout(WORK, scale, side, anchor(0), 180);
                let rail = result.rail;
                // Expanded circle: its bounding box corner is not the circle.
                let shape = result.indicator_shape(0, [SLIDE, 0, 0, 0]);
                let Shape::Ellipse { bounds, .. } = shape else {
                    unreachable!()
                };
                let center = (bounds.x + bounds.width / 2, bounds.y + bounds.height / 2);
                assert!(shape.contains(center.0, center.1), "{scale} {side:?}");
                assert!(!shape.contains(bounds.x, bounds.y), "{scale} {side:?}");
                assert!(
                    !shape.contains(bounds.x + bounds.width - 1, bounds.y + bounds.height - 1),
                    "{scale} {side:?}"
                );
                // Mid-animation (or collapsed), the zone follows the accepted
                // frame, not the final expanded position.
                let collapsed = result.hover_zone(0, [0; MAX_INDICATORS], None);
                let expanded_center = center;
                assert!(
                    !collapsed.contains(expanded_center.0, expanded_center.1),
                    "{scale} {side:?}"
                );
                // The collapsed half circle is clipped by the window.
                let edge_y = rail.y + pixels(INDICATOR_RADIUS, scale);
                let outside_x = match side {
                    Side::Right => rail.x + rail.width,
                    Side::Left => rail.x - 1,
                };
                assert!(!collapsed.contains(outside_x, edge_y), "{scale} {side:?}");
                let inside_x = match side {
                    Side::Right => rail.x + rail.width - 2,
                    Side::Left => rail.x + 1,
                };
                assert!(collapsed.contains(inside_x, edge_y), "{scale} {side:?}");
            }
        }
    }
}

#[cfg(test)]
mod slot_tests {
    use super::*;

    const WORK: Rect = Rect {
        x: 0,
        y: 0,
        width: 1920,
        height: 1040,
    };

    #[test]
    fn a_single_visible_provider_gets_a_single_circle_rail() {
        assert_eq!(rail_height(2), RAIL_HEIGHT);
        assert_eq!(rail_height(1), INDICATOR_DIAMETER);
        let one = layout_with(WORK, 1.25, Side::Right, 1, anchor(0), 180);
        assert_eq!(one.rail.height, 65);
        assert_eq!(one.visible_circles([0; MAX_INDICATORS]).len(), 1);
        assert_eq!(
            layout_with(WORK, 1.0, Side::Left, 2, 0, 0)
                .visible_circles([0; MAX_INDICATORS])
                .len(),
            2
        );
        // Out-of-range counts are clamped.
        assert_eq!(layout_with(WORK, 1.0, Side::Left, 0, 0, 0).rail.height, 52);
        assert_eq!(layout_with(WORK, 1.0, Side::Left, 9, 0, 0).rail.height, 250);
    }

    #[test]
    fn i07_a_narrow_work_area_keeps_the_right_circle_on_the_window_edge() {
        let narrow = Rect { width: 40, ..WORK };
        let result = layout_with(narrow, 1.0, Side::Right, 2, 0, 0);
        assert_eq!(result.rail.width, 40);
        let circle = result.indicator_circles([0; MAX_INDICATORS])[0];
        // Collapsed: the circle center sits on the window's right edge.
        assert_eq!(circle.x + circle.width / 2, result.rail.width);
    }

    #[test]
    fn one_to_four_indicators_are_centered_and_evenly_spaced() {
        for scale in [1.0, 1.25, 1.5] {
            for side in [Side::Left, Side::Right] {
                for slots in 1..=MAX_INDICATORS {
                    let result = layout_with(WORK, scale, side, slots, 0, 0);
                    // Vertically centered in the work area.
                    let above = result.rail.y - WORK.y;
                    let below = WORK.y + WORK.height - (result.rail.y + result.rail.height);
                    assert!((above - below).abs() <= 1, "{scale} {side:?} {slots}");
                    assert_eq!(result.rail.height, pixels(rail_height(slots), scale));
                    // One region circle per visible indicator, evenly spaced.
                    let circles = result.visible_circles([0; MAX_INDICATORS]);
                    assert_eq!(circles.len(), slots);
                    for pair in circles.windows(2) {
                        let step = pair[1].y - pair[0].y;
                        assert!((step - pixels(INDICATOR_STEP, scale)).abs() <= 1);
                    }
                    // Every circle stays inside the rail window.
                    for circle in &circles {
                        // The region pad (1 logical px) may round to 2 physical px.
                        assert!(
                            circle.y >= -2 && circle.y + circle.height <= result.rail.height + 2
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn each_indicator_slides_on_its_own() {
        let result = layout_with(WORK, 1.0, Side::Right, 4, 0, 0);
        let collapsed = result.indicator_circles([0; MAX_INDICATORS]);
        let third = result.indicator_circles([0, 0, SLIDE, 0]);
        for index in [0, 1, 3] {
            assert_eq!(collapsed[index], third[index]);
        }
        assert_eq!(third[2].x, collapsed[2].x - i32::from(SLIDE));
        // Mirrored on the left.
        let left = layout_with(WORK, 1.0, Side::Left, 4, 0, 0);
        let moved = left.indicator_circles([0, 0, 0, SLIDE])[3];
        assert_eq!(
            moved.x,
            left.indicator_circles([0; MAX_INDICATORS])[3].x + i32::from(SLIDE)
        );
    }

    #[test]
    fn secondary_monitors_with_negative_origins_keep_the_rail_on_screen() {
        let secondary = Rect {
            x: -1920,
            y: -300,
            width: 1920,
            height: 1080,
        };
        for side in [Side::Left, Side::Right] {
            let result = layout_with(secondary, 1.25, side, 4, anchor(3), 200);
            assert!(result.rail.x >= secondary.x);
            assert!(result.rail.x + result.rail.width <= secondary.x + secondary.width);
            assert!(result.rail.y >= secondary.y);
            assert!(result.rail.y + result.rail.height <= secondary.y + secondary.height);
            assert!(result.summary.y >= secondary.y);
            assert!(result.summary.y + result.summary.height <= secondary.y + secondary.height);
            // The summary of the last indicator faces inward.
            match side {
                Side::Left => assert!(result.summary.x >= result.rail.x + result.rail.width),
                Side::Right => assert!(result.summary.x + result.summary.width <= result.rail.x),
            }
        }
    }

    #[test]
    fn a_short_work_area_shows_only_the_indicators_that_fit() {
        assert_eq!(capacity(1040, 1.0), 4);
        // 250 logical pixels fit four exactly; one less fits three.
        assert_eq!(capacity(250, 1.0), 4);
        assert_eq!(capacity(249, 1.0), 3);
        assert_eq!(capacity(375, 1.5), 4);
        assert_eq!(capacity(374, 1.5), 3);
        assert_eq!(capacity(10, 1.0), 1);
        assert_eq!(capacity(0, f64::NAN), 1);
        let short = Rect {
            height: 200,
            ..WORK
        };
        let result = layout_with(short, 1.0, Side::Right, 4, 0, 0);
        assert_eq!(result.visible_circles([0; MAX_INDICATORS]).len(), 3);
        assert!(result.rail.height <= short.height);
    }
}
