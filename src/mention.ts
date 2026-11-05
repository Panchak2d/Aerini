/**
 * `@` mentions in a text field. A mention starts with an `@` the user just typed at the
 * start of a word; an `@` after any non-space character (an email address, a Discord
 * `<@id>` mention) is left alone.
 */

/** True when the character before `caret` is an `@` that begins a word. */
export function isMentionStart(value: string, caret: number): boolean {
  if (caret < 1 || value[caret - 1] !== "@") return false;
  return caret === 1 || /\s/.test(value[caret - 2]);
}

/** Text typed after the `@` at `start`, up to `caret`; null when the mention no longer applies. */
export function mentionQuery(value: string, start: number, caret: number): string | null {
  if (value[start] !== "@" || caret <= start) return null;
  const query = value.slice(start + 1, caret);
  if (/[\r\n]/.test(query) || /^\s/.test(query)) return null;
  return query;
}
