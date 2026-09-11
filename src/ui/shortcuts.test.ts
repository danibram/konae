import { describe, expect, test } from "bun:test";
import { shortcutFor } from "./shortcuts";

const chord = (key: string, inTextEntry = false) => ({ key, metaKey: true, ctrlKey: false, inTextEntry });

describe("editor shortcuts", () => {
  test("a chord needs a modifier", () => {
    expect(shortcutFor({ key: "1", metaKey: false, ctrlKey: false, inTextEntry: false })).toBeNull();
    expect(shortcutFor({ key: "1", metaKey: false, ctrlKey: true, inTextEntry: false })).toEqual({ kind: "reveal", panel: "code" });
  });

  test("the number keys reveal the four editor panels", () => {
    expect(shortcutFor(chord("1"))).toEqual({ kind: "reveal", panel: "code" });
    expect(shortcutFor(chord("4"))).toEqual({ kind: "reveal", panel: "auth" });
    expect(shortcutFor(chord("5"))).toBeNull();
    expect(shortcutFor(chord("0"))).toBeNull();
  });

  test("the letter keys fold a panel away, whatever the shift state", () => {
    expect(shortcutFor(chord("b"))).toEqual({ kind: "fold", panel: "explorer" });
    expect(shortcutFor(chord("J"))).toEqual({ kind: "fold", panel: "agent" });
    expect(shortcutFor(chord("k"))).toBeNull();
  });

  /**
   * The bug this pins: ⌘↵ in the SQL console ran the query *and* reloaded the
   * preview, because the window listener fired even though CodeMirror had
   * already handled the chord.
   */
  test("Return belongs to the surface being typed in", () => {
    expect(shortcutFor(chord("Enter"))).toEqual({ kind: "run" });
    expect(shortcutFor(chord("Enter", true))).toBeNull();
    // Panel chords stay global: they are how a panel is reached from anywhere.
    expect(shortcutFor(chord("2", true))).toEqual({ kind: "reveal", panel: "data" });
  });
});
