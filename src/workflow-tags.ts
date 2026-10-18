export const MAX_TAG_LENGTH = 32;
export const MAX_TAGS_PER_WORKFLOW = 20;

export interface TaggedItem {
  name: string;
  tags?: unknown;
}

export interface TagIndexEntry {
  label: string;
  count: number;
}

export type TagIndex = Map<string, TagIndexEntry>;

export interface TagFacet {
  key: string;
  label: string;
  count: number;
  selected: boolean;
}

export interface TagEdit {
  add: string[];
  remove: string[];
}

/** Returns the cleaned tag, or null when nothing usable is left. Over-long
 *  input is truncated rather than rejected so dirty on-disk data still
 *  yields a usable label. */
export function normalizeTag(raw: unknown): string | null {
  if (typeof raw !== "string") return null;
  const cleaned = raw
    .normalize("NFC")
    .replace(/\s+/g, " ")
    .replace(/[\u0000-\u001f\u007f-\u009f,]/g, "")
    .trim();
  if (!cleaned) return null;
  const clipped = Array.from(cleaned).slice(0, MAX_TAG_LENGTH).join("").trim();
  return clipped || null;
}

export function tagKey(tag: string): string {
  return tag.toLocaleLowerCase();
}

function cleanTags(raw: unknown, library: TagIndex | undefined, limit: number): string[] {
  if (!Array.isArray(raw)) return [];
  const seen = new Set<string>();
  const out: string[] = [];
  for (const item of raw) {
    const tag = normalizeTag(item);
    if (!tag) continue;
    const key = tagKey(tag);
    if (seen.has(key)) continue;
    seen.add(key);
    out.push(library?.get(key)?.label ?? tag);
    if (out.length >= limit) break;
  }
  return out;
}

/** Cleans a tag list: drops non-strings and empties, dedupes by
 *  case-insensitive key keeping first-seen casing (or the library's casing
 *  when `library` knows the key), and caps at MAX_TAGS_PER_WORKFLOW. */
export function normalizeTags(raw: unknown, library?: TagIndex): string[] {
  return cleanTags(raw, library, MAX_TAGS_PER_WORKFLOW);
}

/** Counts workflows per tag key across the whole library. The label is the
 *  first casing seen in `items` order, so pass the list in a stable order. */
export function buildTagIndex(items: readonly TaggedItem[]): TagIndex {
  const index: TagIndex = new Map();
  for (const item of items) {
    for (const tag of cleanTags(item.tags, undefined, Infinity)) {
      const key = tagKey(tag);
      const entry = index.get(key);
      if (entry) entry.count++;
      else index.set(key, { label: tag, count: 1 });
    }
  }
  return index;
}

function tagKeysOf(item: TaggedItem): Set<string> {
  return new Set(cleanTags(item.tags, undefined, Infinity).map(tagKey));
}

/** Case-insensitive substring match against the name or any single tag.
 *  Tags are matched one by one, never as a joined string, so a query cannot
 *  match across a tag boundary. */
export function matchesQuery(item: TaggedItem, query: string): boolean {
  const q = query.trim().toLocaleLowerCase();
  if (!q) return true;
  if (item.name.toLocaleLowerCase().includes(q)) return true;
  for (const tag of cleanTags(item.tags, undefined, Infinity)) {
    if (tagKey(tag).includes(q)) return true;
  }
  return false;
}

/** AND semantics: the item must carry every selected tag. */
export function matchesTags(item: TaggedItem, selectedKeys: ReadonlySet<string>): boolean {
  if (selectedKeys.size === 0) return true;
  const keys = tagKeysOf(item);
  for (const key of selectedKeys) if (!keys.has(key)) return false;
  return true;
}

export function applyListFilters<T extends TaggedItem>(
  items: readonly T[],
  query: string,
  selectedKeys: ReadonlySet<string>,
): T[] {
  return items.filter(i => matchesQuery(i, query) && matchesTags(i, selectedKeys));
}

/** Selected keys that still exist somewhere in the library. */
export function pruneSelectedTags(index: TagIndex, selectedKeys: ReadonlySet<string>): Set<string> {
  return new Set([...selectedKeys].filter(k => index.has(k)));
}

/** Filter-bar chips. An unselected chip's count is how many workflows the
 *  list would show if it were clicked; unselected tags that would show none
 *  are omitted. Selected tags always appear. Order: selected first, then
 *  count descending, then label. */
export function buildTagFacets(
  items: readonly TaggedItem[],
  query: string,
  selectedKeys: ReadonlySet<string>,
  index: TagIndex = buildTagIndex(items),
): TagFacet[] {
  const visible = applyListFilters(items, query, selectedKeys);
  const counts = new Map<string, number>();
  for (const item of visible) {
    for (const key of tagKeysOf(item)) counts.set(key, (counts.get(key) ?? 0) + 1);
  }
  const facets: TagFacet[] = [];
  for (const [key, count] of counts) {
    facets.push({ key, label: index.get(key)?.label ?? key, count, selected: selectedKeys.has(key) });
  }
  for (const key of selectedKeys) {
    if (!counts.has(key) && index.has(key)) {
      facets.push({ key, label: index.get(key)!.label, count: 0, selected: true });
    }
  }
  return facets.sort((a, b) =>
    Number(b.selected) - Number(a.selected) ||
    b.count - a.count ||
    a.label.localeCompare(b.label));
}

/** Autocomplete candidates: library tags not in `excludeKeys`, prefix
 *  matches before other substring matches, then count descending, then label. */
export function suggestTags(
  index: TagIndex,
  excludeKeys: ReadonlySet<string>,
  input: string,
  limit: number,
): TagIndexEntry[] {
  const q = (normalizeTag(input) ?? "").toLocaleLowerCase();
  const rows: Array<{ key: string; entry: TagIndexEntry; prefix: boolean }> = [];
  for (const [key, entry] of index) {
    if (excludeKeys.has(key)) continue;
    if (q && !key.includes(q)) continue;
    rows.push({ key, entry, prefix: !q || key.startsWith(q) });
  }
  rows.sort((a, b) =>
    Number(b.prefix) - Number(a.prefix) ||
    b.entry.count - a.entry.count ||
    a.entry.label.localeCompare(b.entry.label));
  return rows.slice(0, limit).map(r => r.entry);
}

export interface TagEditResult {
  tags: string[];
  changed: boolean;
  /** Tags from `edit.add` that were dropped because the list was full. */
  skippedForLimit: number;
}

/** Applies add/remove sets to one workflow's tags. Removal wins over nothing,
 *  add skips tags already present, and tags unique to this workflow are
 *  never touched unless named in `remove`. */
export function applyTagEdit(current: unknown, edit: TagEdit, library?: TagIndex): TagEditResult {
  const before = cleanTags(current, undefined, Infinity);
  const removeKeys = new Set(edit.remove.map(t => normalizeTag(t)).filter((t): t is string => !!t).map(tagKey));
  const next = before.filter(t => !removeKeys.has(tagKey(t)));
  const have = new Set(next.map(tagKey));
  let skippedForLimit = 0;
  for (const raw of edit.add) {
    const tag = normalizeTag(raw);
    if (!tag) continue;
    const key = tagKey(tag);
    if (have.has(key)) continue;
    if (next.length >= MAX_TAGS_PER_WORKFLOW) { skippedForLimit++; continue; }
    have.add(key);
    next.push(library?.get(key)?.label ?? tag);
  }
  const changed = next.length !== before.length || next.some((t, i) => t !== before[i]);
  return { tags: next, changed, skippedForLimit };
}
