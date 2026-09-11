/**
 * The workshop layout: a tree of tabbed groups.
 *
 * The model follows the one every large editor converged on. A *stack* is a
 * group of panels shown as tabs, one active at a time; a *split* arranges
 * stacks (or other splits) in a direction with parallel sizes. Dropping a panel
 * on the middle of a stack adds a tab; dropping it on an edge splits. Opening a
 * panel that is already somewhere reveals it instead of duplicating it.
 *
 * Pure data and pure transforms: no React, no DOM, no knowledge of what a panel
 * contains. Every operation is total, because this state is also read back from
 * localStorage where an older or hand-edited value can turn up.
 */

export type PanelId = "explorer" | "code" | "data" | "files" | "auth" | "preview" | "agent";

export type StackNode = { type: "stack"; panels: PanelId[]; active: number };
export type SplitNode = {
  type: "split";
  direction: "row" | "column";
  children: LayoutNode[];
  /** Percentages parallel to `children`, summing to 100. */
  sizes: number[];
};
export type LayoutNode = StackNode | SplitNode;

export type LayoutState = {
  root: LayoutNode | null;
  /** Panels not on screen. They are closed, not destroyed: opening restores them. */
  closed: PanelId[];
  /** Where a newly opened panel lands: the stack holding this one. */
  focused: PanelId | null;
};

/** Middle of a stack means "add a tab"; an edge means "split there". */
export type Side = "left" | "right" | "top" | "bottom";
export type DropZone = Side | "center";
export type Placement = {
  target: PanelId;
  zone: DropZone;
  /**
   * Where a "center" drop lands relative to `target`. A tab strip is a list, so
   * "join this group" is not enough to say what the owner aimed at: dropping on
   * the left half of a tab must put the panel before it, and only `before` can
   * express the first position at all.
   */
  before?: boolean;
};

export const panelIds: PanelId[] = ["explorer", "code", "data", "files", "auth", "preview", "agent"];

export const panelLabels: Record<PanelId, string> = {
  explorer: "Explorer",
  code: "Code",
  data: "Data",
  files: "Files",
  auth: "Auth",
  preview: "Preview",
  agent: "Agent",
};

/** Panels that behave like documents: opening one goes to the focused group. */
export const editorPanels: PanelId[] = ["code", "data", "files", "auth"];

/** The smallest share a panel may be squeezed to, as a percentage of its split. */
export const minShare = 8;

export function stack(panels: PanelId[], active = 0): StackNode {
  return { type: "stack", panels, active: clampIndex(active, panels.length) };
}

export function split(direction: "row" | "column", children: LayoutNode[], sizes?: number[]): SplitNode {
  return { type: "split", direction, children, sizes: sizes ?? evenSizes(children.length) };
}

function clampIndex(index: number, length: number): number {
  if (length === 0) return 0;
  return Math.max(0, Math.min(Math.round(index) || 0, length - 1));
}

function evenSizes(count: number): number[] {
  if (count <= 0) return [];
  const each = round(100 / count);
  const sizes = Array.from({ length: count }, () => each);
  sizes[count - 1] = round(100 - each * (count - 1));
  return sizes;
}

function round(value: number): number {
  return Math.round(value * 100) / 100;
}

export function defaultLayout(): LayoutState {
  return {
    root: split(
      "row",
      [stack(["explorer"]), stack(["code"]), split("column", [stack(["preview"]), stack(["agent"])])],
      [15, 51, 34],
    ),
    closed: ["data", "files", "auth"],
    focused: "code",
  };
}

export function panelsIn(node: LayoutNode | null): PanelId[] {
  if (!node) return [];
  return node.type === "stack" ? [...node.panels] : node.children.flatMap(panelsIn);
}

/** The active tab of every group, which is what the owner can actually see. */
export function visiblePanels(node: LayoutNode | null): PanelId[] {
  if (!node) return [];
  if (node.type === "stack") {
    const panel = node.panels[node.active];
    return panel ? [panel] : [];
  }
  return node.children.flatMap(visiblePanels);
}

export function stackPathOf(node: LayoutNode | null, panel: PanelId, prefix: number[] = []): number[] | null {
  if (!node) return null;
  if (node.type === "stack") return node.panels.includes(panel) ? prefix : null;
  for (const [index, child] of node.children.entries()) {
    const found = stackPathOf(child, panel, [...prefix, index]);
    if (found) return found;
  }
  return null;
}

export function nodeAt(node: LayoutNode | null, path: number[]): LayoutNode | null {
  let current = node;
  for (const index of path) {
    if (!current || current.type !== "split") return null;
    current = current.children[index] ?? null;
  }
  return current;
}

/** Removes a panel, dropping any group left empty and collapsing lone splits. */
function detach(node: LayoutNode | null, panel: PanelId): LayoutNode | null {
  if (!node) return null;
  if (node.type === "stack") {
    if (!node.panels.includes(panel)) return node;
    const panels = node.panels.filter((candidate) => candidate !== panel);
    if (panels.length === 0) return null;
    const removedIndex = node.panels.indexOf(panel);
    return stack(panels, removedIndex <= node.active ? node.active - 1 : node.active);
  }

  const kept: LayoutNode[] = [];
  const sizes: number[] = [];
  node.children.forEach((child, index) => {
    const next = detach(child, panel);
    if (!next) return;
    kept.push(next);
    sizes.push(node.sizes[index] ?? 0);
  });
  if (kept.length === 0) return null;
  if (kept.length === 1) return kept[0];
  const total = sizes.reduce((sum, size) => sum + size, 0) || 1;
  return split(node.direction, kept, sizes.map((size) => round((size / total) * 100)));
}

const sideAxis: Record<Side, "row" | "column"> = { left: "row", right: "row", top: "column", bottom: "column" };
const sideFirst: Record<Side, boolean> = { left: true, right: false, top: true, bottom: false };

function mapStack(node: LayoutNode, panel: PanelId, replace: (stack: StackNode) => LayoutNode): LayoutNode {
  if (node.type === "stack") return node.panels.includes(panel) ? replace(node) : node;
  return { ...node, children: node.children.map((child) => mapStack(child, panel, replace)) };
}

/**
 * Drops `panel` onto `placement.target`: as a tab when the zone is the middle,
 * as a new group beside it when the zone is an edge.
 */
export function movePanel(state: LayoutState, panel: PanelId, placement: Placement): LayoutState {
  if (panel === placement.target) return state;
  const withoutPanel = detach(state.root, panel);
  const closed = state.closed.filter((id) => id !== panel);
  if (!withoutPanel) return { root: stack([panel]), closed, focused: panel };
  if (!panelsIn(withoutPanel).includes(placement.target)) return state;

  const root = mapStack(withoutPanel, placement.target, (group) => {
    if (placement.zone === "center") {
      const at = group.panels.indexOf(placement.target) + (placement.before ? 0 : 1);
      const panels = [...group.panels.slice(0, at), panel, ...group.panels.slice(at)];
      return stack(panels, at);
    }
    const moved = stack([panel]);
    const pair = sideFirst[placement.zone] ? [moved, group] : [group, moved];
    return split(sideAxis[placement.zone], pair);
  });

  return { root, closed, focused: panel };
}

/**
 * Reveals a panel: if it is already somewhere it becomes the active tab of its
 * group, otherwise it opens as a tab in the focused group. Splitting stays an
 * explicit gesture, exactly as in the editors this model comes from.
 */
export function openPanel(state: LayoutState, panel: PanelId): LayoutState {
  const path = stackPathOf(state.root, panel);
  if (path) {
    const group = nodeAt(state.root, path) as StackNode;
    return {
      ...state,
      root: writeNode(state.root!, path, stack(group.panels, group.panels.indexOf(panel))),
      focused: panel,
    };
  }

  const host = hostPanelFor(state);
  if (!host) return { root: stack([panel]), closed: state.closed.filter((id) => id !== panel), focused: panel };
  return movePanel(state, panel, { target: host, zone: "center" });
}

/** The panel whose group receives a newly opened one. */
function hostPanelFor(state: LayoutState): PanelId | null {
  const open = panelsIn(state.root);
  if (open.length === 0) return null;
  const preferred = [state.focused, ...editorPanels].find(
    (candidate) => candidate && open.includes(candidate),
  );
  return (preferred as PanelId | undefined) ?? open[0];
}

export function closePanel(state: LayoutState, panel: PanelId): LayoutState {
  if (!panelsIn(state.root).includes(panel)) return state;
  const root = detach(state.root, panel);
  const focused = state.focused === panel ? visiblePanels(root)[0] ?? null : state.focused;
  return { root, closed: [...state.closed.filter((id) => id !== panel), panel], focused };
}

/**
 * Folds a panel away, or brings it back. "Back" means visible: a panel that is
 * open but behind another tab is revealed, not closed and reopened somewhere
 * else — the fold is about what the owner can see.
 */
export function togglePanel(state: LayoutState, panel: PanelId): LayoutState {
  return visiblePanels(state.root).includes(panel) ? closePanel(state, panel) : openPanel(state, panel);
}

export function isOpen(state: LayoutState, panel: PanelId): boolean {
  return panelsIn(state.root).includes(panel);
}

/** True when the panel is the one its group is currently showing. */
export function isVisible(state: LayoutState, panel: PanelId): boolean {
  return visiblePanels(state.root).includes(panel);
}

export function activateTab(state: LayoutState, path: number[], index: number): LayoutState {
  const group = nodeAt(state.root, path);
  if (!state.root || !group || group.type !== "stack") return state;
  const next = stack(group.panels, index);
  return { ...state, root: writeNode(state.root, path, next), focused: next.panels[next.active] ?? state.focused };
}

/**
 * Drags the handle between children `index` and `index + 1`: `share` is the
 * first one's portion of the pair, and every other child of the split keeps its
 * size exactly. The floor is applied here, against the pair, because a share
 * that would put a neighbour under it has to stop at the neighbour — leaving it
 * to `shareOut` pays the difference out of children the gesture never touched.
 */
export function resizeSplit(state: LayoutState, path: number[], index: number, share: number): LayoutState {
  const node = nodeAt(state.root, path);
  if (!state.root || !node || node.type !== "split") return state;
  if (!Number.isFinite(share) || index < 0 || index + 1 >= node.children.length) return state;

  const pair = (node.sizes[index] ?? 0) + (node.sizes[index + 1] ?? 0);
  if (!(pair > 0)) return state;
  const floor = Math.min(minShare, 100 / node.children.length);
  const lowest = Math.min(floor / pair, 0.5);
  const clamped = Math.min(Math.max(share, lowest), 1 - lowest);

  const sizes = [...node.sizes];
  sizes[index] = round(clamped * pair);
  sizes[index + 1] = round(pair - sizes[index]);
  return setSizes(state, path, sizes);
}

export function setSizes(state: LayoutState, path: number[], sizes: number[]): LayoutState {
  const node = nodeAt(state.root, path);
  if (!state.root || !node || node.type !== "split" || node.children.length !== sizes.length) return state;
  return { ...state, root: writeNode(state.root, path, { ...node, sizes: shareOut(sizes) }) };
}

/**
 * Turns raw sizes into percentages summing to 100 with no child below the
 * floor. The floor has to be applied after normalising, not before, or dividing
 * by the new total pushes small children straight back under it.
 */
function shareOut(sizes: number[]): number[] {
  if (sizes.length === 0) return [];
  const floor = Math.min(minShare, 100 / sizes.length);
  const total = sizes.reduce((sum, size) => sum + Math.max(0, size), 0) || 1;
  const scaled = sizes.map((size) => (Math.max(0, size) / total) * 100);

  const deficit = scaled.reduce((sum, size) => sum + Math.max(0, floor - size), 0);
  const spare = scaled.reduce((sum, size) => sum + Math.max(0, size - floor), 0) || 1;
  const settled = scaled.map((size) =>
    size < floor ? floor : size - ((size - floor) / spare) * deficit,
  );

  const rounded = settled.map(round);
  rounded[rounded.length - 1] = round(100 - rounded.slice(0, -1).reduce((sum, size) => sum + size, 0));
  return rounded;
}

function writeNode(root: LayoutNode, path: number[], node: LayoutNode): LayoutNode {
  if (path.length === 0) return node;
  if (root.type !== "split") return root;
  const [index, ...rest] = path;
  return {
    ...root,
    children: root.children.map((child, at) => (at === index ? writeNode(child, rest, node) : child)),
  };
}

/** Layouts written before panels were tabbed; kept so an upgrade never wipes one. */
const renamed: Record<string, PanelId> = { workbench: "code", database: "data" };

/**
 * Repairs anything read from storage: unknown panels are dropped, a panel that
 * appears twice keeps its first place, and a panel the stored layout never heard
 * of is closed rather than lost.
 */
export function normalizeLayout(value: unknown): LayoutState {
  if (!value || typeof value !== "object") return defaultLayout();
  const input = value as Record<string, unknown>;
  const seen = new Set<PanelId>();

  const readPanel = (raw: unknown): PanelId | null => {
    const name = typeof raw === "string" ? (renamed[raw] ?? raw) : "";
    const panel = name as PanelId;
    if (!panelIds.includes(panel) || seen.has(panel)) return null;
    seen.add(panel);
    return panel;
  };

  const clean = (node: unknown): LayoutNode | null => {
    if (!node || typeof node !== "object") return null;
    const candidate = node as Record<string, unknown>;

    if (candidate.type === "leaf") {
      const panel = readPanel(candidate.panel);
      return panel ? stack([panel]) : null;
    }
    if (candidate.type === "stack") {
      const panels = Array.isArray(candidate.panels)
        ? candidate.panels.map(readPanel).filter((panel): panel is PanelId => Boolean(panel))
        : [];
      return panels.length > 0 ? stack(panels, Number(candidate.active) || 0) : null;
    }
    if (candidate.type !== "split" || !Array.isArray(candidate.children)) return null;
    const stored = candidate.children;

    const children: LayoutNode[] = [];
    const sizes: number[] = [];
    stored.forEach((child, index) => {
      const next = clean(child);
      if (!next) return;
      children.push(next);
      const size = Number((candidate.sizes as number[] | undefined)?.[index]);
      sizes.push(Number.isFinite(size) && size > 0 ? size : 100 / stored.length);
    });
    if (children.length === 0) return null;
    if (children.length === 1) return children[0];
    const total = sizes.reduce((sum, size) => sum + size, 0) || 1;
    const direction = candidate.direction === "column" ? "column" : "row";
    return split(direction, children, sizes.map((size) => round((size / total) * 100)));
  };

  const root = clean(input.root);
  if (!root) return defaultLayout();

  const storedClosed = Array.isArray(input.closed) ? input.closed : Array.isArray(input.hidden) ? input.hidden : [];
  const closed = storedClosed
    .map((raw) => (typeof raw === "string" ? (renamed[raw] ?? raw) : "") as PanelId)
    .filter((panel) => panelIds.includes(panel) && !seen.has(panel));
  for (const panel of closed) seen.add(panel);
  for (const panel of panelIds) if (!seen.has(panel)) closed.push(panel);

  const focusedRaw = typeof input.focused === "string" ? (renamed[input.focused] ?? input.focused) : "";
  const focused = panelsIn(root).includes(focusedRaw as PanelId)
    ? (focusedRaw as PanelId)
    : visiblePanels(root)[0] ?? null;

  return { root, closed: [...new Set(closed)], focused };
}

const storageKey = "littlebox:layout";

export function loadLayout(storage: Pick<Storage, "getItem"> = localStorage): LayoutState {
  try {
    const raw = storage.getItem(storageKey);
    return raw ? normalizeLayout(JSON.parse(raw)) : defaultLayout();
  } catch {
    return defaultLayout();
  }
}

export function saveLayout(state: LayoutState, storage: Pick<Storage, "setItem"> = localStorage): void {
  try {
    storage.setItem(storageKey, JSON.stringify(state));
  } catch {
    /* a workshop that cannot persist its layout still works */
  }
}

/**
 * Which drop zone a pointer is over: the middle of a group means "tab", the
 * outer band means "split on that side". The band is a quarter of each axis,
 * which is wide enough to hit without aiming.
 */
export function dropZoneAt(
  box: { left: number; top: number; width: number; height: number },
  x: number,
  y: number,
): DropZone {
  const fromLeft = (x - box.left) / box.width;
  const fromTop = (y - box.top) / box.height;
  const edges: Array<[Side, number]> = [
    ["left", fromLeft],
    ["right", 1 - fromLeft],
    ["top", fromTop],
    ["bottom", 1 - fromTop],
  ];
  const [side, distance] = edges.reduce((closest, entry) => (entry[1] < closest[1] ? entry : closest));
  return distance < 0.25 ? side : "center";
}
