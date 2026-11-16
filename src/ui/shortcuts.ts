import type { PanelId } from "./layout";

export type ShortcutAction =
  | { kind: "run" }
  /** Bring a panel forward in its group, opening it if it was closed. */
  | { kind: "reveal"; panel: PanelId }
  /** Fold a panel away, or bring it back if it is already folded. */
  | { kind: "fold"; panel: PanelId };

export type ShortcutEvent = {
  key: string;
  metaKey: boolean;
  ctrlKey: boolean;
  inTextEntry: boolean;
};

const revealed: PanelId[] = ["code", "data", "files", "auth"];
const folds: Record<string, PanelId> = { b: "explorer", j: "agent", u: "preview" };

export function shortcutFor(event: ShortcutEvent): ShortcutAction | null {
  if (!event.metaKey && !event.ctrlKey) return null;
  if (event.key === "Enter") return event.inTextEntry ? null : { kind: "run" };

  const revealedPanel = revealed[Number(event.key) - 1];
  if (revealedPanel) return { kind: "reveal", panel: revealedPanel };

  const folded = folds[event.key.toLowerCase()];
  return folded ? { kind: "fold", panel: folded } : null;
}

const textEntrySelector = "input, textarea, select, [contenteditable=''], [contenteditable='true'], .cm-editor";

export function inTextEntry(target: EventTarget | null): boolean {
  return target instanceof Element && target.closest(textEntrySelector) !== null;
}
