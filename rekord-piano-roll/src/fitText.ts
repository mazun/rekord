import { tick } from "svelte";
import type { Action } from "svelte/action";

// Glyph advances don't scale perfectly linearly with font-size (hinting,
// rounding), so shrink to slightly under the exact fit to avoid a 1-2px spill.
const SAFETY = 0.97;

/**
 * Shrinks the element's font-size, only when needed, so its text fits within
 * its parent's width. Short text keeps the CSS font-size. The parent should
 * have a fixed width and height, so shrinking never changes the layout.
 *
 * Usage: `<span use:fitText={text}>{text}</span>` -- the parameter is only
 * the trigger for re-measuring; pass whatever the text depends on.
 */
export const fitText: Action<HTMLElement, unknown> = (node) => {
  const fit = () => {
    const box = node.parentElement;
    if (!box) return;
    node.style.fontSize = "";
    const natural = node.getBoundingClientRect().width;
    const available = box.clientWidth;
    if (natural > available && available > 0) {
      node.style.fontSize = `${(available / natural) * SAFETY * 100}%`;
    }
  };
  fit();
  // update() runs before Svelte writes the new text into the DOM, so measure
  // after the flush.
  return { update: () => void tick().then(fit) };
};
