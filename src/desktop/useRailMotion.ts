import { useEffect, useRef, useState } from "react";
import type { Provider } from "../usage/model";
import {
  getRailPlacement,
  onRailPlacement,
  setHover,
  setRailExpansion,
} from "./commands";
import type { RailPlacement } from "./commands";
import { collapsedWidth, collapsedWidths, expandedWidth } from "./railGeometry";
import type { RailWidths } from "./railGeometry";

const motionMs = 190;

function sameWidths(first: RailWidths, second: RailWidths) {
  return first[0] === second[0] && first[1] === second[1];
}

export function useRailMotion() {
  const [frame, setFrame] = useState<{
    placement: RailPlacement;
    widths: RailWidths;
  } | null>(null);
  const hoverHandler = useRef<((provider: Provider | null) => void) | null>(
    null,
  );

  useEffect(() => {
    let placement: RailPlacement | undefined;
    let widths = collapsedWidths;
    let target = collapsedWidths;
    let hovered: Provider | null = null;
    let transition: { from: RailWidths; start: number } | undefined;
    let request = 0;
    let pending = false;
    let disposed = false;
    const reducedMotion = window.matchMedia("(prefers-reduced-motion: reduce)");

    function schedule() {
      if (!disposed && !request && !pending && !sameWidths(widths, target)) {
        request = requestAnimationFrame(tick);
      }
    }

    function tick(now: number) {
      request = 0;
      const view = placement;
      if (!view || sameWidths(widths, target)) return;
      transition ??= { from: widths, start: now };
      const elapsed = Math.min(1, (now - transition.start) / motionMs);
      const easing = reducedMotion.matches ? 1 : 1 - (1 - elapsed) ** 3;
      const next: RailWidths = [
        Math.round(
          transition.from[0] + (target[0] - transition.from[0]) * easing,
        ),
        Math.round(
          transition.from[1] + (target[1] - transition.from[1]) * easing,
        ),
      ];
      if (sameWidths(widths, next)) {
        schedule();
        return;
      }

      // Keep at most one native update in flight. Only draw widths that Windows
      // accepted, so the pointer region tracks the shape through the animation.
      pending = true;
      void setRailExpansion(view.generation, next)
        .then((accepted) => {
          if (disposed || placement !== view) return;
          if (accepted) {
            widths = next;
            setFrame({ placement: view, widths });
          } else {
            target = widths;
          }
        })
        .catch((error: unknown) => {
          if (placement === view) target = widths;
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
      hovered = null;
      transition = undefined;
      widths = target = collapsedWidths;
      setFrame({ placement: next, widths });
    }

    hoverHandler.current = (provider) => {
      if (!placement || hovered === provider) return;
      hovered = provider;
      target = [
        provider === "codex" ? expandedWidth : collapsedWidth,
        provider === "claudeCode" ? expandedWidth : collapsedWidth,
      ];
      transition = undefined;
      schedule();
      void setHover(placement.generation, provider).catch(console.error);
    };

    const listener = onRailPlacement(acceptPlacement);
    void listener
      .then(() => getRailPlacement())
      .then(acceptPlacement)
      .catch(console.error);

    return () => {
      disposed = true;
      hoverHandler.current = null;
      cancelAnimationFrame(request);
      void listener.then((unlisten) => unlisten()).catch(console.error);
    };
  }, []);

  return {
    frame,
    onHover: (provider: Provider | null) => hoverHandler.current?.(provider),
  };
}
