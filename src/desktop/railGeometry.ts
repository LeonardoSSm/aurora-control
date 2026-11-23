// Logical dimensions shared with src-tauri/src/windows/position.rs.
export const indicatorDiameter = 52;
export const indicatorRadius = indicatorDiameter / 2;
export const expandedInset = 6;
/** How far the circle center travels inward when expanded. */
export const slide = indicatorRadius + expandedInset;
export const indicatorGap = 14;
export const indicatorStep = indicatorDiameter + indicatorGap;
export const railWidth = slide + indicatorRadius + 2;
export const railHeight = indicatorStep + indicatorDiameter;

/** Radius of the disc. Half a pixel inside the region, for an anti-aliased edge. */
export const discRadius = indicatorRadius - 0.5;
export const arcWidth = 5.5;
/** Radius of the gauge stroke center. */
export const arcRadius = discRadius - 1 - arcWidth / 2 - 1;

export type Side = "left" | "right";
export type RailShifts = readonly [number, number];

export const collapsedShifts: RailShifts = [0, 0];

/**
 * The gauge is the semicircle that faces the inside of the screen: the half
 * left visible when the indicator is collapsed. It fills from the bottom to the
 * top along that half, so 50% always reaches the innermost point, on either
 * side. `fraction` runs from 0 to 1.
 */
export function arcPoint(
  side: Side,
  cx: number,
  cy: number,
  radius: number,
  fraction: number,
) {
  const angle = Math.PI * Math.min(1, Math.max(0, fraction));
  const inward = side === "right" ? -1 : 1;
  return {
    x: cx + inward * radius * Math.sin(angle),
    y: cy + radius * Math.cos(angle),
  };
}

const fixed = (value: number) => Number(value.toFixed(2)).toString();

/** Path from the bottom of the gauge to `fraction` of it, or null for zero. */
export function arcPath(
  side: Side,
  cx: number,
  cy: number,
  radius: number,
  fraction: number,
) {
  const end = Math.min(1, Math.max(0, fraction));
  if (end <= 0) return null;
  // Bottom to top through the inward side: clockwise on the right, counter-
  // clockwise on the left. Split at the middle so no segment spans 180°.
  const sweep = side === "right" ? 1 : 0;
  const start = arcPoint(side, cx, cy, radius, 0);
  const stops = end > 0.5 ? [0.5, end] : [end];
  const segments = stops.map((stop) => {
    const point = arcPoint(side, cx, cy, radius, stop);
    return `A ${fixed(radius)} ${fixed(radius)} 0 0 ${sweep} ${fixed(point.x)} ${fixed(point.y)}`;
  });
  return `M ${fixed(start.x)} ${fixed(start.y)} ${segments.join(" ")}`;
}

/**
 * Geometry of one indicator in its own `railWidth × indicatorDiameter` box.
 * `shift` is how far the circle has slid inward, from 0 (half hidden) to
 * `slide` (fully visible).
 */
export function ringGeometry(side: Side, shift: number) {
  const offset = Math.min(slide, Math.max(0, shift));
  const progress = offset / slide;
  const inward = side === "right" ? -1 : 1;
  const cx = side === "right" ? railWidth - offset : offset;
  const cy = indicatorRadius;
  // Collapsed, the value sits in the middle of the visible half; expanded, it
  // moves to the center of the circle.
  const collapsedTextX = cx + inward * 10;
  return {
    cx,
    cy,
    progress,
    textX: collapsedTextX + (cx - collapsedTextX) * progress,
  };
}
