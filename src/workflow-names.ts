export interface NamedWorkflow { id: string; name: string; }

const norm = (s: string): string => s.trim().toLowerCase();

/** True when a workflow other than `exceptId` already uses `name`. Case-insensitive, like most file managers. */
export function isWorkflowNameTaken(
  workflows: ReadonlyArray<NamedWorkflow> | null | undefined,
  name: string,
  exceptId?: string,
): boolean {
  if (!workflows) return false;
  const target = norm(name);
  return workflows.some(w => w.id !== exceptId && norm(w.name) === target);
}

/** `base` if free, otherwise the first of `base (2)`, `base (3)`, ... that is free. */
export function uniqueWorkflowName(existingNames: ReadonlyArray<string>, base: string): string {
  const taken = new Set(existingNames.map(norm));
  if (!taken.has(norm(base))) return base;
  for (let n = 2; ; n++) {
    const candidate = `${base} (${n})`;
    if (!taken.has(norm(candidate))) return candidate;
  }
}
