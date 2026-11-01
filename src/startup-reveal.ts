/** Longest the window stays hidden at launch, so a slow load can never leave it invisible. */
export const REVEAL_CAP_MS = 800;

export interface Reveal {
  /** Shows the window at once. Only the first call, from here or the cap timer, has an effect. */
  fire(): void;
  /** Shows the window after the browser has painted twice, so the first frame is on screen before the window is. */
  afterPaint(): void;
}

export function createReveal(show: () => void, capMs: number = REVEAL_CAP_MS): Reveal {
  let shown = false;
  const fire = (): void => {
    if (shown) return;
    shown = true;
    clearTimeout(timer);
    show();
  };
  const timer = setTimeout(fire, capMs);
  return { fire, afterPaint: () => requestAnimationFrame(() => requestAnimationFrame(fire)) };
}
