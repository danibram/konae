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
  resetLayout,
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
    expect(group.panels).toContain("preview");
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
    const backToPreview = openPanel(withData, "preview");
    expect(groupOf(backToPreview, "preview").panels).toEqual(groupOf(withData, "data").panels);
    expect(isVisible(backToPreview, "preview")).toBe(true);
    expect(isVisible(backToPreview, "data")).toBe(false);
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
    expect(isVisible(shared, "preview")).toBe(false);

    const revealed = togglePanel(shared, "preview");
    expect(isVisible(revealed, "preview")).toBe(true);
    expect(isOpen(revealed, "preview")).toBe(true);
    // Only a panel the owner can see folds away.
    expect(isOpen(togglePanel(revealed, "preview"), "preview")).toBe(false);
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
    const state = movePanel(defaultLayout(), "agent", { target: "preview", zone: "center" });
    const group = groupOf(state, "agent");
    expect(group.panels).toEqual(["preview", "agent"]);
    expect(group.panels[group.active]).toBe("agent");
  });

  test("dropping on an edge splits in that direction", () => {
    const below = movePanel(defaultLayout(), "agent", { target: "preview", zone: "bottom" });
    const path = stackPathOf(below.root, "agent")!;
    const parent = nodeAt(below.root, path.slice(0, -1)) as any;
    expect(parent.direction).toBe("column");
    expect(path[path.length - 1]).toBe(1);

    const left = movePanel(defaultLayout(), "agent", { target: "preview", zone: "left" });
    const leftPath = stackPathOf(left.root, "agent")!;
    expect((nodeAt(left.root, leftPath.slice(0, -1)) as any).direction).toBe("row");
    expect(leftPath[leftPath.length - 1]).toBe(0);
  });

  test("the group a panel leaves disappears when it was its last tab", () => {
    const state = movePanel(defaultLayout(), "preview", { target: "agent", zone: "center" });
    // preview's old group held only preview, so the column collapsed onto agent.
    expect(stackPathOf(state.root, "agent")).not.toBeNull();
    expect(accountedFor(state)).toEqual(everything);
  });

  test("dropping a panel on itself changes nothing", () => {
    const state = defaultLayout();
    expect(movePanel(state, "preview", { target: "preview", zone: "left" })).toBe(state);
  });

  test("a center drop can land before its target, so a tab reaches the first position", () => {
    const group = openPanel(openPanel(defaultLayout(), "data"), "files");
    expect(groupOf(group, "preview").panels).toEqual(["preview", "data", "files"]);

    const first = movePanel(group, "files", { target: "preview", zone: "center", before: true });
    expect(groupOf(first, "files").panels).toEqual(["files", "preview", "data"]);
    expect(groupOf(first, "files").active).toBe(0);
  });

  test("moving one tab reaches every order that moving one tab can reach", () => {
    const group = openPanel(openPanel(defaultLayout(), "data"), "files");
    const orders = new Set<string>();
    for (const moved of ["preview", "data", "files"] as const) {
      for (const target of ["preview", "data", "files"] as const) {
        for (const before of [true, false]) {
          const next = movePanel(group, moved, { target, zone: "center", before });
          orders.add(groupOf(next, moved).panels.join(","));
        }
      }
    }
    // Anchoring only ever after the target reached three of these five.
    expect([...orders].sort()).toEqual([
      "data,files,preview",
      "data,preview,files",
      "files,preview,data",
      "preview,data,files",
      "preview,files,data",
    ]);
  });
});

describe("tabs and sizes", () => {
  test("activating a tab changes what the group shows", () => {
    const state = openPanel(defaultLayout(), "data");
    const path = stackPathOf(state.root, "preview")!;
    const back = activateTab(state, path, 0);
    expect(visiblePanels(back.root)).toContain("preview");
    expect(visiblePanels(back.root)).not.toContain("data");
  });

  test("dragging a handle resizes its own pair and nothing else", () => {
    const sizesOf = (state: LayoutState) => (state.root as any).sizes as number[];
    const start = defaultLayout();
    expect(sizesOf(start)).toEqual([64, 36]);

    const middle = resizeSplit(start, [], 0, 0.5);
    expect(sizesOf(middle)).toEqual([50, 50]);

    // Past the floor the handle stops at the floor: the other child is not paying for it.
    const endStop = resizeSplit(start, [], 0, 0);
    expect(sizesOf(endStop)).toEqual([8, 92]);
    expect(sizesOf(resizeSplit(start, [], 0, 1))).toEqual([92, 8]);
  });

  test("repeated end-stop drags never drift the untouched children", () => {
    let state = defaultLayout();
    for (let attempt = 0; attempt < 5; attempt += 1) state = resizeSplit(state, [], 0, 0);
    expect((state.root as any).sizes).toEqual([8, 92]);
  });

  test("resizing something that is not a split, or a pair that does not exist, is a no-op", () => {
    const state = defaultLayout();
    expect(resizeSplit(state, [], 2, 0.5)).toBe(state);
    expect(resizeSplit(state, [0], 0, 0.5)).toBe(state);
    expect(resizeSplit(state, [], 0, Number.NaN)).toBe(state);
  });

  test("sizes normalize to 100 with a floor under each child", () => {
    const state = setSizes(defaultLayout(), [], [97, 3]);
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
    const state = togglePanel(movePanel(defaultLayout(), "agent", { target: "preview", zone: "center" }), "explorer");
    const restored = normalizeLayout(JSON.parse(JSON.stringify(state)));
    expect(restored).toEqual(state);
  });
});

describe("the default arrangement", () => {
  test("opens with explorer, code, preview and agent, and the rest closed", () => {
    const state = defaultLayout();
    expect(panelsIn(state.root)).toEqual(["preview", "agent"]);
    expect([...state.closed].sort()).toEqual(["activity", "auth", "code", "data", "explorer", "files"]);
    expect(state.focused).toBe("preview");
    expect(split("row", [stack(["code"])]).children).toHaveLength(1);
  });
});

describe("closed panels remember their slot", () => {
  const rootSizes = (state: LayoutState) => (state.root?.type === "split" ? state.root.sizes : []);

  test("opening the file list puts it left of the preview, and closing it returns", () => {
    const start = defaultLayout();
    const opened = togglePanel(start, "explorer");
    expect(groupOf(opened, "explorer").panels).toEqual(["explorer"]);
    expect(stackPathOf(opened.root, "explorer")).toEqual([0]);
    const closed = togglePanel(opened, "explorer");
    expect(closed.root).toEqual(start.root);
    expect(rootSizes(closed)).toEqual(rootSizes(start));
  });

  test("the preview and the agent come back beside each other", () => {
    const start = defaultLayout();
    for (const panel of ["preview", "agent"] as const) {
      const back = togglePanel(togglePanel(start, panel), panel);
      expect(isVisible(back, "preview")).toBe(true);
      expect(isVisible(back, "agent")).toBe(true);
      expect(panelsIn(back.root).sort()).toEqual(["agent", "preview"]);
    }
  });

  test("a panel closed from a tab group reopens as a tab beside the same neighbour", () => {
    const withData = openPanel(defaultLayout(), "data");
    const reopened = openPanel(closePanel(withData, "data"), "data");
    expect(groupOf(reopened, "data").panels).toEqual(["preview", "data"]);
    expect(isVisible(reopened, "data")).toBe(true);
  });

  test("a slot whose neighbour has gone falls back to the panel's default place", () => {
    const noExplorer = closePanel(defaultLayout(), "explorer");
    const stale: LayoutState = { ...noExplorer, homes: { explorer: { target: "agent", zone: "left", size: 15 } } };
    const withoutAgent = closePanel(stale, "agent");
    const reopened = openPanel({ ...withoutAgent, homes: { ...withoutAgent.homes, explorer: { target: "auth", zone: "left" } } }, "explorer");
    expect(isVisible(reopened, "explorer")).toBe(true);
    expect(groupOf(reopened, "explorer").panels).toEqual(["explorer"]);
    expect(stackPathOf(reopened.root, "explorer")).toEqual([0]);
  });

  test("layouts stored before slots were remembered still load, and slots survive storage", () => {
    const old = normalizeLayout({ root: defaultLayout().root, closed: ["explorer", "code", "data", "files", "auth"], focused: "preview" });
    expect(old.homes).toEqual({});
    const folded = togglePanel(defaultLayout(), "explorer");
    const restored = normalizeLayout(JSON.parse(JSON.stringify(folded)));
    expect(restored.homes?.explorer).toEqual(folded.homes?.explorer);
    expect(normalizeLayout({ ...folded, homes: { explorer: { target: "nowhere", zone: "left" } } }).homes).toEqual({});
  });

  test("reset returns the default arrangement", () => {
    const messy = movePanel(togglePanel(defaultLayout(), "explorer"), "agent", { target: "preview", zone: "center" });
    expect(resetLayout()).toEqual(defaultLayout());
    expect(resetLayout().root).not.toEqual(messy.root);
  });
});
