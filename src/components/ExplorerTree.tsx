import { ChevronRight, Folder, FolderOpen, type LucideIcon } from "lucide-react";
import { useEffect, useMemo, useState } from "react";
import { ancestorsOf, buildTree, type TreeNode } from "./project-tree";

type Props = {
  files: string[];
  directories: string[];
  activeFile: string;
  selectedPath: string;
  iconFor: (path: string) => LucideIcon;
  noteFor?: (path: string) => string;
  onOpenFile: (path: string) => void;
  onSelect: (path: string) => void;
};

export function ExplorerTree({ files, directories, activeFile, selectedPath, iconFor, noteFor, onOpenFile, onSelect }: Props) {
  const nodes = useMemo(() => buildTree(files, directories), [files, directories]);
  const [collapsed, setCollapsed] = useState<Set<string>>(() => new Set());

  const reveal = (path: string) => {
    const open = new Set(ancestorsOf(path));
    setCollapsed((current) => {
      if (![...current].some((folder) => open.has(folder))) return current;
      return new Set([...current].filter((folder) => !open.has(folder)));
    });
  };

  useEffect(() => reveal(activeFile), [activeFile]);
  useEffect(() => reveal(selectedPath), [selectedPath]);

  const toggle = (path: string) => setCollapsed((current) => {
    const next = new Set(current);
    if (next.has(path)) next.delete(path);
    else next.add(path);
    return next;
  });

  const render = (node: TreeNode, depth: number) => {
    const indent = { paddingLeft: `${8 + depth * 14}px` };
    if (node.kind === "directory") {
      const open = !collapsed.has(node.path);
      return (
        <div key={`dir:${node.path}`} className="tree-group">
          <button
            className={`tree-directory ${selectedPath === node.path ? "selected" : ""}`}
            style={indent}
            title={node.path}
            aria-expanded={open}
            onClick={() => {
              toggle(node.path);
              onSelect(node.path);
            }}
          >
            <ChevronRight size={12} className={`tree-chevron ${open ? "is-open" : ""}`} />
            {open ? <FolderOpen size={13} /> : <Folder size={13} />}
            <span className="tree-label">{node.name}</span>
          </button>
          {open && node.children.map((child) => render(child, depth + 1))}
        </div>
      );
    }
    const Icon = iconFor(node.path);
    return (
      <button
        key={node.path}
        className={`tree-file ${activeFile === node.path ? "active" : ""} ${selectedPath === node.path ? "selected" : ""}`}
        style={{ paddingLeft: `${8 + depth * 14 + 16}px` }}
        title={node.path}
        aria-current={activeFile === node.path ? "page" : undefined}
        onClick={() => onOpenFile(node.path)}
      >
        <Icon size={13} />
        <span className="tree-label">{node.name}</span>
        {noteFor?.(node.path) && <span className="tree-note" aria-hidden="true">{noteFor(node.path)}</span>}
      </button>
    );
  };

  return <div className="explorer-tree">{nodes.map((node) => render(node, 0))}</div>;
}
