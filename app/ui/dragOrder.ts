import { useRef, useState } from "react";

/** How far a press has to move before it's a drag and not a click. */
const DRAG_STARTS_AT = 4;

/** Rows put in order by dragging, or with Option and the arrow keys. The one being dragged
 * stays with the pointer and the others slide out of its way. Every row is the same height,
 * and the list's element holds nothing but them. */
export function useDragOrder<K>(ids: K[], onMoved: (ids: K[]) => void) {
  const list = useRef<HTMLDivElement>(null);
  /** The row being dragged: where it was, where it would land, how far it has moved, and how
   * far apart rows are. */
  const [drag, setDrag] = useState<{ id: K; from: number; to: number; dy: number; step: number } | null>(null);
  const justDragged = useRef(false);

  const start = (e: React.PointerEvent, id: K) => {
    justDragged.current = false;
    if (e.button !== 0 || e.pointerType !== "mouse") return;
    const startedAt = e.clientY;
    const from = ids.indexOf(id);
    const rows = [...(list.current?.children ?? [])];
    // From one row's top to the next, gap included.
    const step = rows.length > 1 ? rows[1].getBoundingClientRect().top - rows[0].getBoundingClientRect().top : 0;
    let to = from;
    const move = (ev: PointerEvent) => {
      if (!justDragged.current && Math.abs(ev.clientY - startedAt) < DRAG_STARTS_AT) return;
      justDragged.current = true;
      // With the pointer, but no further than the first or last place.
      const dy = Math.min(Math.max(ev.clientY - startedAt, -from * step), (ids.length - 1 - from) * step);
      to = step ? from + Math.round(dy / step) : from;
      setDrag({ id, from, to, dy, step });
    };
    const up = () => {
      window.removeEventListener("pointermove", move);
      window.removeEventListener("pointerup", up);
      setDrag(null);
      if (!justDragged.current || to === from) return;
      const moved = ids.filter((other) => other !== id);
      moved.splice(to, 0, id);
      onMoved(moved);
    };
    window.addEventListener("pointermove", move);
    window.addEventListener("pointerup", up);
  };

  const byKey = (e: React.KeyboardEvent, id: K) => {
    const step = e.altKey ? { ArrowUp: -1, ArrowDown: 1 }[e.key] : undefined;
    if (!step) return;
    e.preventDefault();
    const to = ids.indexOf(id) + step;
    if (to < 0 || to >= ids.length) return;
    const moved = ids.filter((other) => other !== id);
    moved.splice(to, 0, id);
    onMoved(moved);
  };

  /** How far a row sits from its place while one is dragged. */
  const shift = (id: K, i: number) => {
    if (!drag) return 0;
    if (id === drag.id) return drag.dy;
    if (drag.from < i && i <= drag.to) return -drag.step;
    if (drag.to <= i && i < drag.from) return drag.step;
    return 0;
  };

  return {
    list,
    /** For the list's element, so the others can slide while one is dragged. */
    listClass: drag ? "nav-dragging" : undefined,
    /** What each row needs, its place in the list given. */
    row: (id: K, i: number) => ({
      moving: drag?.id === id,
      style: drag ? { transform: `translateY(${shift(id, i)}px)` } : undefined,
      "aria-keyshortcuts": "Alt+ArrowUp Alt+ArrowDown",
      onPointerDown: (e: React.PointerEvent) => start(e, id),
      onKeyDown: (e: React.KeyboardEvent) => byKey(e, id),
    }),
    /** A press that ended a drag isn't a click. */
    clicked: () => {
      if (!justDragged.current) return true;
      justDragged.current = false;
      return false;
    },
  };
}
