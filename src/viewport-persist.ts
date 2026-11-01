export interface ViewportSnapshot { panX: number; panY: number; zoom: number }

export interface ViewportPersistOptions {
  currentId: () => string | null;
  read: () => ViewportSnapshot;
  write: (key: string, value: string) => void;
  delayMs?: number;
}

export const viewportKey = (id: string): string => `aerini_viewport_${id}`;

/**
 * Remembers each workflow's pan and zoom. A burst of changes becomes one
 * write, and the snapshot is taken when the change is noted so it is saved
 * under the workflow it belongs to even if the user has switched since.
 */
export function createViewportPersist(opts: ViewportPersistOptions): { note(): void; flush(): void } {
  const delay = opts.delayMs ?? 200;
  let pending: { id: string; vp: ViewportSnapshot } | null = null;
  let timer: ReturnType<typeof setTimeout> | null = null;

  function flush(): void {
    if (timer !== null) { clearTimeout(timer); timer = null; }
    if (!pending) return;
    const { id, vp } = pending;
    pending = null;
    opts.write(viewportKey(id), JSON.stringify(vp));
  }

  function note(): void {
    const id = opts.currentId();
    if (!id) return;
    const { panX, panY, zoom } = opts.read();
    pending = { id, vp: { panX, panY, zoom } };
    if (timer !== null) clearTimeout(timer);
    timer = setTimeout(flush, delay);
  }

  return { note, flush };
}
