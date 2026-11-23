// Logical dimensions shared with src-tauri/src/windows/position.rs.
export const indicatorDiameter = 76;
export const collapsedWidth = indicatorDiameter / 2;
export const expandedWidth = 64;
export type Side = "left" | "right";
export type RailWidths = readonly [number, number];

export const collapsedWidths: RailWidths = [collapsedWidth, collapsedWidth];

export function ringGeometry(side: Side, width: number) {
  const inward = side === "right" ? -1 : 1;
  const edge = side === "right" ? expandedWidth : 0;
  const centerY = indicatorDiameter / 2;
  const rx = width - 4.5;
  const ry = centerY - 4.5;
  const endX = (edge + inward * rx * Math.sin(Math.PI / 25)).toFixed(2);
  const endY = ry * Math.cos(Math.PI / 25);
  const sweep = side === "right" ? 0 : 1;
  const arc = `M ${endX} ${(centerY - endY).toFixed(2)} A ${rx} ${ry} 0 0 ${sweep} ${endX} ${(centerY + endY).toFixed(2)}`;

  return { edge, centerY, arc, textX: edge + inward * width * 0.46 };
}
