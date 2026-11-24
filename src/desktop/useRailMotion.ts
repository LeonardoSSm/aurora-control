import { useEffect, useRef, useState } from "react";
import type { Provider } from "../usage/model";
import {
  getRailPlacement,
  onRailActive,
  onRailPlacement,
  onRailPointerReset,
  railPointer,
  setRailMotion,
} from "./commands";
import type { RailActive, RailPlacement } from "./commands";
import { collapsedShifts, slide } from "./railGeometry";
import type { RailShifts } from "./railGeometry";

const openMs = 230;
const closeMs = 190;

export function same(first: RailShifts, second: RailShifts) {
  return (
    first.length === second.length &&
    first.every((value, index) => value === second[index])
  );
}

/** Offsets that expand `slot` (or nothing, for -1) and collapse the rest. */
export function targetShifts(slot: number): RailShifts {
  return collapsedShifts.map((_, index) => (index === slot ? slide : 0));
}

/** One animation step of every slot from `from` toward `target`. */
export function stepShifts(
  from: RailShifts,
  target: RailShifts,
  eased: number,
): RailShifts {
  return target.map((goal, index) => {
    const start = from[index] ?? 0;
    return Math.round(start + (goal - start) * eased);
  });
}

const easeOut = (t: number) => 1 - (1 - t) ** 3;
const easeInOut = (t: number) =>
  t < 0.5 ? 4 * t ** 3 : 1 - (-2 * t + 2) ** 3 / 2;

/**
 * Drives the slide of each indicator. Rust decides which indicator is active
 * (`rail-active`); this hook animates toward it and only draws offsets that
 * Windows accepted, so the input region tracks the shape on every frame.
 */
export function useRailMotion() {
  const [frame, setFrame] = useState<{
    placement: RailPlacement;
    shifts: RailShifts;
    active: Provider | null;
  } | null>(null);
  const pointerHandler = useRef<((provider: Provider | null) => void) | null>(
    null,
  );

  useEffect(() => {
    let placement: RailPlacement | undefined;
    let active: Provider | null = null;
    let shifts = collapsedShifts;
    let target = collapsedShifts;
    let transition: { from: RailShifts; start: number } | undefined;
    let request = 0;
    let pending = false;
    let disposed = false;
    let reported: Provider | null = null;
    const reducedMotion = window.matchMedia("(prefers-reduced-motion: reduce)");

    function schedule() {
      if (!disposed && !request && !pending && !same(shifts, target)) {
        request = requestAnimationFrame(tick);
      }
    }

    function tick(now: number) {
      request = 0;
      const view = placement;
      if (!view || same(shifts, target)) return;
      transition ??= { from: shifts, start: now };
      const opening = target.some((value) => value > 0);
      const duration = opening ? openMs : closeMs;
      const elapsed = Math.min(1, (now - transition.start) / duration);
      const eased = reducedMotion.matches
        ? 1
        : (opening ? easeOut : easeInOut)(elapsed);
      const from = transition.from;
      const next = stepShifts(from, target, eased);
      if (same(shifts, next)) {
        schedule();
        return;
      }

      // At most one native update in flight. Only draw accepted offsets.
      pending = true;
      void setRailMotion(view.generation, next)
        .then((accepted) => {
          if (disposed || placement !== view) return;
          if (accepted) {
            shifts = next;
            setFrame({ placement: view, shifts, active });
          } else {
            target = shifts;
          }
        })
        .catch((error: unknown) => {
          if (placement === view) target = shifts;
          console.error(error);
        })
        .finally(() => {
          pending = false;
          schedule();
        });
    }

    function acceptPlacement(next: RailPlacement) {
      if (disposed || (placement && next.generation <= placement.generation)) {
        return;
      }
      placement = next;
      active = reported = null;
      transition = undefined;
      shifts = target = collapsedShifts;
      setFrame({ placement: next, shifts, active });
    }

    function acceptActive(next: RailActive) {
      if (disposed || !placement || next.generation !== placement.generation) {
        return;
      }
      active = next.provider;
      // A closed panel may follow a lost pointer-leave: report again on the
      // next movement so hovering the same indicator reopens it.
      if (!active) reported = null;
      // Shifts are per visible slot, in the order Rust placed them.
      const slot = active ? placement.providers.indexOf(active) : -1;
      target = targetShifts(slot);
      transition = undefined;
      setFrame({ placement, shifts, active });
      schedule();
    }

    pointerHandler.current = (provider) => {
      if (!placement || reported === provider) return;
      reported = provider;
      void railPointer(placement.generation, provider).catch(console.error);
    };

    const listeners = [
      onRailPlacement(acceptPlacement),
      // Rust found the cursor elsewhere when the intent elapsed (a lost
      // pointerleave): the next movement over an indicator reports again.
      onRailPointerReset((generation) => {
        if (placement?.generation === generation) reported = null;
      }),
      onRailActive(acceptActive),
    ];
    void Promise.all(listeners)
      .then(() => getRailPlacement())
      .then(acceptPlacement)
      .catch(console.error);

    return () => {
      disposed = true;
      pointerHandler.current = null;
      cancelAnimationFrame(request);
      for (const listener of listeners) {
        void listener.then((unlisten) => unlisten()).catch(console.error);
      }
    };
  }, []);

  return {
    frame,
    onPointer: (provider: Provider | null) =>
      pointerHandler.current?.(provider),
  };
}
