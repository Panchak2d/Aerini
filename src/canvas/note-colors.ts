export const NOTE_COLORS = ["default", "yellow", "blue", "green", "red"] as const;

export type NoteColorName = (typeof NOTE_COLORS)[number];

export const NOTE_COLOR_VAR: Record<NoteColorName, string> = {
  default: "var(--color-border)", yellow: "var(--color-warning)", blue: "var(--color-action-nav)",
  green: "var(--color-action-run)", red: "var(--color-error)",
};

export function noteColorLabel(name: NoteColorName): string {
  return name[0].toUpperCase() + name.slice(1);
}

/** The palette name stored in a note's config, with anything unknown read as the default. */
export function noteColorOf(config: Record<string, unknown>): NoteColorName {
  const v = config["color"];
  return NOTE_COLORS.find(c => c === v) ?? "default";
}
