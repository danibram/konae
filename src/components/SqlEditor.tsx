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
          EditorView.theme({
            "&": { height: "100%", backgroundColor: "#24251f", color: "#e8e6dc", fontSize: "11px" },
            ".cm-content": { caretColor: "#c8f135", fontFamily: "JetBrains Mono Variable, monospace", padding: "12px 0" },
            ".cm-line": { padding: "0 10px" },
            ".cm-gutters": { backgroundColor: "#20211c", color: "#62645c", borderRight: "1px solid #383a32" },
            ".cm-activeLine, .cm-activeLineGutter": { backgroundColor: "#2d2f28" },
            ".cm-selectionBackground, &.cm-focused .cm-selectionBackground": { backgroundColor: "#4e6035 !important" },
            ".cm-cursor": { borderLeftColor: "#c8f135" },
            ".cm-tooltip": { backgroundColor: "#f1efe7", color: "#24251f", border: "1px solid #24251f" },
            ".cm-tooltip-autocomplete ul li[aria-selected]": { backgroundColor: "#c8f135", color: "#24251f" },
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
