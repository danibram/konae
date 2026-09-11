import { autocompletion } from "@codemirror/autocomplete";
import { defaultKeymap, history, historyKeymap } from "@codemirror/commands";
import { sql } from "@codemirror/lang-sql";
import { EditorState } from "@codemirror/state";
import { EditorView, keymap, lineNumbers } from "@codemirror/view";
import { useEffect, useRef } from "react";

type Props = {
  value: string;
  schema: Record<string, string[]>;
  onChange: (value: string) => void;
  onExecute: () => void;
};

export function SqlEditor({ value, schema, onChange, onExecute }: Props) {
  const containerRef = useRef<HTMLDivElement>(null);
  const viewRef = useRef<EditorView | null>(null);
  const onChangeRef = useRef(onChange);
  const onExecuteRef = useRef(onExecute);
  onChangeRef.current = onChange;
  onExecuteRef.current = onExecute;

  useEffect(() => {
    if (!containerRef.current) return;
    const view = new EditorView({
      parent: containerRef.current,
      state: EditorState.create({
        doc: value,
        extensions: [
          lineNumbers(),
          history(),
          sql({ schema }),
          autocompletion(),
          keymap.of([
            { key: "Mod-Enter", run: () => { onExecuteRef.current(); return true; } },
            ...defaultKeymap,
            ...historyKeymap,
          ]),
          EditorView.lineWrapping,
          EditorView.updateListener.of((update) => {
            if (update.docChanged) onChangeRef.current(update.state.doc.toString());
          }),
          // The dark reading surface matches the code editor; popups use the design tokens.
          EditorView.theme({
            "&": { height: "100%", backgroundColor: "#20232b", color: "#dce0e9", fontSize: "11px" },
            ".cm-content": { caretColor: "#a4abff", fontFamily: "var(--ds-mono)", padding: "12px 0" },
            ".cm-line": { padding: "0 10px" },
            ".cm-gutters": { backgroundColor: "#1b1e25", color: "#858c9c", borderRight: "1px solid #2a2e39" },
            ".cm-activeLine, .cm-activeLineGutter": { backgroundColor: "#272b35" },
            ".cm-selectionBackground, &.cm-focused .cm-selectionBackground": { backgroundColor: "#3b4170 !important" },
            ".cm-cursor": { borderLeftColor: "#a4abff" },
            ".cm-tooltip": { backgroundColor: "var(--ds-surface)", color: "var(--ds-text)", border: "1px solid var(--ds-border)", borderRadius: "var(--ds-radius-sm)" },
            ".cm-tooltip-autocomplete ul li[aria-selected]": { backgroundColor: "var(--ds-accent)", color: "#ffffff" },
          }),
        ],
      }),
    });
    viewRef.current = view;
    return () => { view.destroy(); viewRef.current = null; };
  }, [schema]);

  useEffect(() => {
    const view = viewRef.current;
    if (!view || view.state.doc.toString() === value) return;
    view.dispatch({ changes: { from: 0, to: view.state.doc.length, insert: value } });
  }, [value]);

  return <div ref={containerRef} className="sql-editor" aria-label="SQL editor" />;
}
