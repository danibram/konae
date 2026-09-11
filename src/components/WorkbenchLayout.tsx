import { X } from "lucide-react";
import {
  useState,
  type DragEvent as ReactDragEvent,
  type KeyboardEvent as ReactKeyboardEvent,
  type ReactNode,
} from "react";
import {
  activateTab,
  closePanel,
  dropZoneAt,
  movePanel,
  panelLabels,
  panelsIn,
  resizeSplit,
  type DropZone,
  type LayoutNode,
  type LayoutState,
  type PanelId,
} from "../ui/layout";
import "./workbench-layout.css";

export type PanelRender = {
  id: PanelId;
  title: string;
  icon: ReactNode;
  /** Rendered in the group's toolbar while this panel is the active tab. */
  actions?: ReactNode;
  body: ReactNode;
};

type Props = {
  layout: LayoutState;
  onLayoutChange: (layout: LayoutState) => void;
  panels: PanelRender[];
};

type DropTarget = { panel: PanelId; zone: DropZone; before?: boolean };

/** The frame's public handle on a tab: whoever reveals a panel can also focus it. */
export function tabElementId(panel: PanelId): string {
  return `tab-${panel}`;
}

function panelElementId(panel: PanelId): string {
  return `panel-${panel}`;
}

/** Moves keyboard focus onto a panel's tab, so revealing a panel never drops focus on the body. */
export function focusPanelTab(panel: PanelId): void {
  document.getElementById(tabElementId(panel))?.focus();
}

/**
 * Paints the layout tree: splits become rows or columns with a handle between
 * each pair, groups become tab strips. Dragging a tab onto the middle of a group
 * joins it, onto its tab strip places it at that position, and onto an edge
 * splits. The shell never looks inside a panel.
 */
export function WorkbenchLayout({ layout, onLayoutChange, panels }: Props) {
  const [dragging, setDragging] = useState<PanelId | null>(null);
  const [target, setTarget] = useState<DropTarget | null>(null);
  const byId = new Map(panels.map((panel) => [panel.id, panel]));

  function endDrag() {
    setDragging(null);
    setTarget(null);
  }

  function drop() {
    if (dragging && target) {
      onLayoutChange(movePanel(layout, dragging, { target: target.panel, zone: target.zone, before: target.before }));
    }
    endDrag();
  }

  function renderNode(node: LayoutNode, path: number[]): ReactNode {
    if (node.type === "stack") return renderGroup(node.panels, node.active, path);
    return (
      <div className={`split split-${node.direction}`}>
        {node.children.map((child, index) => (
          // Keyed by the panels it holds, not by its position: reshaping a split
          // must not re-key the subtrees that survived it, or React rebuilds them
          // and the preview iframe reloads the mini-app it was running.
          <div
            className="split-slot"
            key={panelsIn(child).join(",")}
            style={{ flexBasis: `${node.sizes[index]}%` }}
          >
            {renderNode(child, [...path, index])}
            {index < node.children.length - 1 && (
              <Handle
                direction={node.direction}
                share={pairShare(node.sizes, index)}
                onResize={(share) => onLayoutChange(resizeSplit(layout, path, index, share))}
              />
            )}
          </div>
        ))}
      </div>
    );
  }

  function renderGroup(members: PanelId[], active: number, path: number[]): ReactNode {
    const current = members[active];
    const actions = current ? byId.get(current)?.actions : undefined;
    const hit = target && members.includes(target.panel) ? target : null;

    function onTabKeyDown(event: ReactKeyboardEvent<HTMLButtonElement>, index: number) {
      const moves: Record<string, number | undefined> = {
        ArrowRight: (index + 1) % members.length,
        ArrowLeft: (index - 1 + members.length) % members.length,
        Home: 0,
        End: members.length - 1,
      };
      const next = moves[event.key];
      if (next !== undefined) {
        event.preventDefault();
        onLayoutChange(activateTab(layout, path, next));
        focusPanelTab(members[next]);
        return;
      }
      if (event.key === "Delete" || event.key === "Backspace") {
        event.preventDefault();
        onLayoutChange(closePanel(layout, members[index]));
        // Focus cannot stay on the tab that just went away.
        const neighbour = members[index + 1] ?? members[index - 1];
        if (neighbour) focusPanelTab(neighbour);
      }
    }

    return (
      <section
        className="group"
        data-panels={members.join(",")}
        onDragOver={(event) => {
          if (!dragging) return;
          const spot = dropTargetAt(event, members, dragging);
          if (!spot) return;
          event.preventDefault();
          setTarget(spot);
        }}
        onDragLeave={(event) => {
          if (event.currentTarget.contains(event.relatedTarget as Node)) return;
          setTarget((value) => (value && members.includes(value.panel) ? null : value));
        }}
        onDrop={(event) => {
          if (!dragging) return;
          event.preventDefault();
          drop();
        }}
      >
        <div
          className="group-tabs"
          role="tablist"
          aria-label={`Panel group: ${members.map((panel) => panelLabels[panel]).join(", ")}`}
        >
          {members.map((panel, index) => {
            const tab = byId.get(panel);
            if (!tab) return null;
            const isActive = index === active;
            return (
              // The tab is the drag handle and holds two controls, so it is a
              // presentational wrapper: `role="tab"` belongs on the one control
              // that selects the panel, directly inside the tablist.
              <div
                key={panel}
                role="presentation"
                data-panel={panel}
                className={`tab tab-${panel} ${isActive ? "active" : ""} ${dragging === panel ? "is-dragging" : ""}`}
                draggable
                onDragStart={(event) => {
                  event.dataTransfer.effectAllowed = "move";
                  // Firefox refuses to start a drag without payload.
                  event.dataTransfer.setData("text/plain", panel);
                  setDragging(panel);
                }}
                onDragEnd={endDrag}
              >
                <button
                  type="button"
                  role="tab"
                  id={tabElementId(panel)}
                  className="tab-label"
                  aria-selected={isActive}
                  aria-controls={panelElementId(panel)}
                  // Roving tabindex: the strip is one tab stop and the arrows walk it.
                  tabIndex={isActive ? 0 : -1}
                  onClick={() => onLayoutChange(activateTab(layout, path, index))}
                  onKeyDown={(event) => onTabKeyDown(event, index)}
                >
                  {tab.icon}
                  <span>{tab.title}</span>
                </button>
                <button
                  type="button"
                  className="tab-close"
                  tabIndex={isActive ? 0 : -1}
                  aria-label={`Close ${panelLabels[panel]}`}
                  onClick={() => onLayoutChange(closePanel(layout, panel))}
                >
                  <X size={10} />
                </button>
              </div>
            );
          })}
          <span className="group-tabs-spacer" />
          {actions && <div className="group-actions">{actions}</div>}
        </div>
        <div className="group-body">
          {/* Every tab of the group stays mounted and the inactive ones are hidden:
              unmounting a body would restart whatever it is running — the preview
              iframe reloads the mini-app and loses its state on a tab switch. */}
          {members.map((panel) => {
            const view = byId.get(panel);
            if (!view) return null;
            return (
              <div
                key={panel}
                className="group-page"
                role="tabpanel"
                id={panelElementId(panel)}
                aria-labelledby={tabElementId(panel)}
                hidden={panel !== current}
              >
                {view.body}
              </div>
            );
          })}
        </div>
        {hit && <span className={`drop-shadow drop-${hit.zone}`} aria-hidden="true" />}
      </section>
    );
  }

  return (
    <div className={`workbench ${dragging ? "is-dragging" : ""}`} onDragEnd={endDrag}>
      {layout.root ? renderNode(layout.root, []) : (
        <p className="workbench-empty">No panels open. Use the bar above to open one.</p>
      )}
      {dragging && panelsIn(layout.root).length > 0 && (
        <p className="drag-hint">Middle of a group joins it · an edge splits</p>
      )}
    </div>
  );
}

function pairShare(sizes: number[], index: number): number {
  const pair = (sizes[index] ?? 0) + (sizes[index + 1] ?? 0);
  return pair > 0 ? (sizes[index] ?? 0) / pair : 0.5;
}

/**
 * Where a drag hovering over a group would land. Over the tab strip the answer
 * is a position in the list — which tab the pointer is over, and which half of
 * it — because placing a tab is what a tab strip is for. Over the body it is the
 * older question: the middle joins the group, an edge splits it.
 */
function dropTargetAt(
  event: ReactDragEvent<HTMLElement>,
  members: PanelId[],
  dragging: PanelId,
): DropTarget | null {
  const node = event.target instanceof Element ? event.target : null;
  if (node?.closest(".group-tabs")) return tabStripTarget(node, members, dragging, event.clientX);

  // Anchor on a panel that is not the one being dragged: a tab dropped on the
  // edge of its own group must still split out of it.
  const anchor = members.find((panel) => panel !== dragging);
  if (!anchor) return null;
  const box = event.currentTarget.getBoundingClientRect();
  return { panel: anchor, zone: dropZoneAt(box, event.clientX, event.clientY) };
}

function tabStripTarget(node: Element, members: PanelId[], dragging: PanelId, x: number): DropTarget | null {
  const tab = node.closest<HTMLElement>(".tab");
  const panel = tab?.dataset.panel as PanelId | undefined;

  if (tab && panel && members.includes(panel)) {
    if (panel !== dragging) {
      const box = tab.getBoundingClientRect();
      return { panel, zone: "center", before: x < box.left + box.width / 2 };
    }
    // Hovering its own tab: anchor on the neighbour that leaves the order as it is.
    const index = members.indexOf(panel);
    const previous = members[index - 1];
    if (previous) return { panel: previous, zone: "center", before: false };
    const following = members[index + 1];
    return following ? { panel: following, zone: "center", before: true } : null;
  }

  // Past the last tab: the end of the list.
  const last = [...members].reverse().find((candidate) => candidate !== dragging);
  return last ? { panel: last, zone: "center", before: false } : null;
}

/** Pointer-driven so a drag keeps working over the preview iframe. */
function Handle({
  direction,
  share,
  onResize,
}: {
  direction: "row" | "column";
  /** The first child's portion of this pair, 0..1. */
  share: number;
  onResize: (share: number) => void;
}) {
  const nudge = 0.02;
  const keys: Record<string, number | undefined> = direction === "row"
    ? { ArrowLeft: share - nudge, ArrowRight: share + nudge }
    : { ArrowUp: share - nudge, ArrowDown: share + nudge };

  return (
    <span
      className={`split-handle ${direction}`}
      role="separator"
      tabIndex={0}
      aria-orientation={direction === "row" ? "vertical" : "horizontal"}
      aria-label={direction === "row" ? "Resize columns" : "Resize rows"}
      aria-valuenow={Math.round(share * 100)}
      aria-valuemin={0}
      aria-valuemax={100}
      onKeyDown={(event) => {
        const next = keys[event.key] ?? (event.key === "Home" ? 0 : event.key === "End" ? 1 : undefined);
        if (next === undefined) return;
        event.preventDefault();
        onResize(next);
      }}
      onPointerDown={(event) => {
        const before = event.currentTarget.parentElement;
        const after = before?.nextElementSibling;
        if (!before || !after) return;
        event.currentTarget.setPointerCapture(event.pointerId);
        // The pair being resized spans from the start of this slot to the end of the next.
        const first = before.getBoundingClientRect();
        const second = after.getBoundingClientRect();
        const start = direction === "row" ? first.left : first.top;
        const end = direction === "row" ? second.right : second.bottom;
        const span = Math.max(1, end - start);

        const move = (moveEvent: PointerEvent) => {
          const pointer = direction === "row" ? moveEvent.clientX : moveEvent.clientY;
          onResize((pointer - start) / span);
        };
        /**
         * A gesture does not always end in a pointerup: a touch the browser takes
         * over for scrolling ends in pointercancel, and losing the capture ends it
         * just as finally. Listening for pointerup alone leaves `move` attached, so
         * every later pointer movement anywhere keeps resizing this split.
         */
        const endings = ["pointerup", "pointercancel", "lostpointercapture"] as const;
        const stop = () => {
          window.removeEventListener("pointermove", move);
          for (const ending of endings) window.removeEventListener(ending, stop);
        };
        window.addEventListener("pointermove", move);
        for (const ending of endings) window.addEventListener(ending, stop);
      }}
    />
  );
}
