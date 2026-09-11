import { describe, expect, test } from "bun:test";
import {
  activateTab,
  closePanel,
  defaultLayout,
  dropZoneAt,
  isOpen,
  isVisible,
  loadLayout,
  movePanel,
  nodeAt,
  normalizeLayout,
  openPanel,
  panelIds,
  panelsIn,
  resizeSplit,
  setSizes,
  split,
  stack,
  stackPathOf,
  togglePanel,
  visiblePanels,
  type LayoutState,
  type StackNode,
} from "./layout";

const everything = [...panelIds].sort();
const accountedFor = (state: LayoutState) => [...panelsIn(state.root), ...state.closed].sort();
const groupOf = (state: LayoutState, panel: string) =>
  nodeAt(state.root, stackPathOf(state.root, panel as never)!) as StackNode;

describe("opening panels", () => {
  test("a closed panel opens as a tab in the focused group, not by splitting", () => {
    const state = openPanel(defaultLayout(), "data");
    const group = groupOf(state, "data");
    expect(group.panels).toContain("code");
    expect(group.panels).toContain("data");
    expect(group.panels[group.active]).toBe("data");
    expect(accountedFor(state)).toEqual(everything);
  });

  test("opening a panel that is already there reveals it instead of duplicating", () => {
    const twice = openPanel(openPanel(defaultLayout(), "data"), "data");
    expect(panelsIn(twice.root).filter((panel) => panel === "data")).toHaveLength(1);
    expect(isVisible(twice, "data")).toBe(true);
  });

  test("revealing a background tab switches its group without moving anything", () => {
    const withData = openPanel(defaultLayout(), "data");
    const backToCode = openPanel(withData, "code");
    expect(groupOf(backToCode, "code").panels).toEqual(groupOf(withData, "data").panels);
    expect(isVisible(backToCode, "code")).toBe(true);
    expect(isVisible(backToCode, "data")).toBe(false);
  });

  test("every panel can be opened and closed, including the code editor", () => {
    for (const panel of panelIds) {
      const opened = openPanel(defaultLayout(), panel);
      expect(isOpen(opened, panel)).toBe(true);
      const closed = closePanel(opened, panel);
      expect(isOpen(closed, panel)).toBe(false);
      expect(accountedFor(closed)).toEqual(everything);
    }
  });

  test("folding a panel that is hidden behind a tab reveals it instead of closing it", () => {
    const shared = openPanel(defaultLayout(), "data");
    expect(isVisible(shared, "code")).toBe(false);

    const revealed = togglePanel(shared, "code");
    expect(isVisible(revealed, "code")).toBe(true);
    expect(isOpen(revealed, "code")).toBe(true);
    // Only a panel the owner can see folds away.
    expect(isOpen(togglePanel(revealed, "code"), "code")).toBe(false);
  });

  test("closing the last panel leaves an empty but usable workshop", () => {
    let state = defaultLayout();
    for (const panel of panelsIn(state.root)) state = closePanel(state, panel);
    expect(state.root).toBeNull();
    expect(panelsIn(openPanel(state, "code").root)).toEqual(["code"]);
  });
});

describe("dropping panels", () => {
  test("dropping on the middle adds a tab beside the target", () => {
    const state = movePanel(defaultLayout(), "agent", { target: "code", zone: "center" });
    const group = groupOf(state, "agent");
    expect(group.panels).toEqual(["code", "agent"]);
    expect(group.panels[group.active]).toBe("agent");
  });

  test("dropping on an edge splits in that direction", () => {
    const below = movePanel(defaultLayout(), "agent", { target: "code", zone: "bottom" });
    const path = stackPathOf(below.root, "agent")!;
    const parent = nodeAt(below.root, path.slice(0, -1)) as any;
    expect(parent.direction).toBe("column");
    expect(path[path.length - 1]).toBe(1);

    const left = movePanel(defaultLayout(), "agent", { target: "code", zone: "left" });
    const leftPath = stackPathOf(left.root, "agent")!;
    expect((nodeAt(left.root, leftPath.slice(0, -1)) as any).direction).toBe("row");
    expect(leftPath[leftPath.length - 1]).toBe(0);
  });

  test("the group a panel leaves disappears when it was its last tab", () => {
    const state = movePanel(defaultLayout(), "preview", { target: "code", zone: "center" });
    // preview's old group held only preview, so the column collapsed onto agent.
    expect(stackPathOf(state.root, "agent")).not.toBeNull();
    expect(accountedFor(state)).toEqual(everything);
  });

  test("dropping a panel on itself changes nothing", () => {
    const state = defaultLayout();
    expect(movePanel(state, "code", { target: "code", zone: "left" })).toBe(state);
  });

  test("a center drop can land before its target, so a tab reaches the first position", () => {
    const group = openPanel(openPanel(defaultLayout(), "data"), "files");
    expect(groupOf(group, "code").panels).toEqual(["code", "data", "files"]);

    const first = movePanel(group, "files", { target: "code", zone: "center", before: true });
    expect(groupOf(first, "files").panels).toEqual(["files", "code", "data"]);
    expect(groupOf(first, "files").active).toBe(0);
  });

  test("moving one tab reaches every order that moving one tab can reach", () => {
    const group = openPanel(openPanel(defaultLayout(), "data"), "files");
    const orders = new Set<string>();
    for (const moved of ["code", "data", "files"] as const) {
      for (const target of ["code", "data", "files"] as const) {
        for (const before of [true, false]) {
          const next = movePanel(group, moved, { target, zone: "center", before });
          orders.add(groupOf(next, moved).panels.join(","));
        }
      }
    }
    // Anchoring only ever after the target reached three of these five.
    expect([...orders].sort()).toEqual([
      "code,data,files",
      "code,files,data",
      "data,code,files",
      "data,files,code",
      "files,code,data",
    ]);
  });
});

describe("tabs and sizes", () => {
  test("activating a tab changes what the group shows", () => {
    const state = openPanel(defaultLayout(), "data");
    const path = stackPathOf(state.root, "code")!;
    const back = activateTab(state, path, 0);
    expect(visiblePanels(back.root)).toContain("code");
    expect(visiblePanels(back.root)).not.toContain("data");
  });

  test("dragging a handle resizes its own pair and nothing else", () => {
    const sizesOf = (state: LayoutState) => (state.root as any).sizes as number[];
    const start = defaultLayout();
    expect(sizesOf(start)).toEqual([15, 51, 34]);

    const middle = resizeSplit(start, [], 0, 0.5);
    expect(sizesOf(middle)).toEqual([33, 33, 34]);

    // Past the floor the handle stops at the floor: the third child is not paying for it.
    const endStop = resizeSplit(start, [], 0, 0);
    expect(sizesOf(endStop)[0]).toBe(8);
    expect(sizesOf(endStop)[2]).toBe(34);
    expect(sizesOf(resizeSplit(start, [], 0, 1))[2]).toBe(34);
  });

  test("repeated end-stop drags never drift the untouched children", () => {
    let state = defaultLayout();
    for (let attempt = 0; attempt < 5; attempt += 1) state = resizeSplit(state, [], 0, 0);
    expect((state.root as any).sizes).toEqual([8, 58, 34]);
  });

  test("resizing something that is not a split, or a pair that does not exist, is a no-op", () => {
    const state = defaultLayout();
    expect(resizeSplit(state, [], 2, 0.5)).toBe(state);
    expect(resizeSplit(state, [0], 0, 0.5)).toBe(state);
    expect(resizeSplit(state, [], 0, Number.NaN)).toBe(state);
  });

  test("sizes normalize to 100 with a floor under each child", () => {
    const state = setSizes(defaultLayout(), [], [97, 2, 1]);
    const root = state.root as any;
    expect(Math.round(root.sizes.reduce((a: number, b: number) => a + b, 0))).toBe(100);
    expect(Math.min(...root.sizes)).toBeGreaterThanOrEqual(8);
  });
});

describe("drop zones", () => {
  test("the middle is a tab and the outer band is a split", () => {
    const box = { left: 0, top: 0, width: 100, height: 100 };
    expect(dropZoneAt(box, 50, 50)).toBe("center");
    expect(dropZoneAt(box, 5, 50)).toBe("left");
    expect(dropZoneAt(box, 95, 50)).toBe("right");
    expect(dropZoneAt(box, 50, 5)).toBe("top");
    expect(dropZoneAt(box, 50, 95)).toBe("bottom");
  });
});

describe("stored layouts", () => {
  test("a layout from before tabs is migrated, not discarded", () => {
    const old = {
      root: { type: "split", direction: "row", sizes: [30, 70], children: [
        { type: "leaf", panel: "explorer" },
        { type: "leaf", panel: "workbench" },
      ] },
      hidden: ["agent"],
    };
    const state = normalizeLayout(old);
    expect(panelsIn(state.root)).toEqual(["explorer", "code"]);
    expect(state.closed).toContain("agent");
    expect(accountedFor(state)).toEqual(everything);
  });

  test("unknown and duplicated panels are dropped, missing ones are closed", () => {
    const state = normalizeLayout({
      root: stack(["code", "code", "ghost" as never]),
      closed: ["nope"],
    });
    expect(panelsIn(state.root)).toEqual(["code"]);
    expect(accountedFor(state)).toEqual(everything);
  });

  test("garbage never throws", () => {
    for (const value of [null, 9, "layout", [], { root: { type: "split" } }]) {
      expect(accountedFor(normalizeLayout(value))).toEqual(everything);
    }
    expect(loadLayout({ getItem: () => "{{{" })).toEqual(defaultLayout());
  });

  test("a full round trip through storage keeps the arrangement", () => {
    const state = togglePanel(movePanel(defaultLayout(), "agent", { target: "code", zone: "center" }), "explorer");
    const restored = normalizeLayout(JSON.parse(JSON.stringify(state)));
    expect(restored).toEqual(state);
  });
});

describe("the default arrangement", () => {
  test("opens with explorer, code, preview and agent, and the rest closed", () => {
    const state = defaultLayout();
    expect(panelsIn(state.root)).toEqual(["explorer", "code", "preview", "agent"]);
    expect([...state.closed].sort()).toEqual(["auth", "data", "files"]);
    expect(state.focused).toBe("code");
    expect(split("row", [stack(["code"])]).children).toHaveLength(1);
  });
});
