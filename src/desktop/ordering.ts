// Drops events delivered out of order (audit I03). Rust numbers every
// publication; the WebViews only move forward.

/** Accepts a usage update only when it is newer than the last one shown. */
export function isNewerUpdate(last: number | null, next: number) {
  return last === null || next > last;
}

export interface SelectionOrder {
  /** Hover selection: a new selection always has a higher revision. */
  revision: number;
  /** Data publication within the same selection. */
  sequence: number;
}

/**
 * A summary selection replaces the visible one only when it belongs to a
 * newer selection, or brings newer data for the same selection.
 */
export function isNewerSelection(
  last: SelectionOrder | null,
  next: SelectionOrder,
) {
  if (!last) return true;
  if (next.revision !== last.revision) return next.revision > last.revision;
  return next.sequence > last.sequence;
}
