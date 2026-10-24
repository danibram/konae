import type { MiniAppProject } from "./types";

export type AgentEdit = {
  project: MiniAppProject;
  message: string;
  changedFiles: string[];
};

export function applyLocalAgentEdit(project: MiniAppProject, prompt: string): AgentEdit {
  const normalized = prompt.toLowerCase();
  const next: MiniAppProject = {
    ...project,
    files: { ...project.files },
    updatedAt: Date.now(),
  };

  if (normalized.includes("azul") || normalized.includes("blue")) {
    next.files["styles.css"] = `${project.files["styles.css"]}\n/* Agent edit: ink blue accent */\n:root { --ds-accent: #3157d5; --ds-accent-hover: #2745b0; --ds-accent-soft: #dfe6fb; --ds-on-accent: #ffffff; }\n`;
    return {
      project: next,
      message: "Proposed change to styles.css: the accent becomes ink blue. Nothing is saved until you apply it.",
      changedFiles: ["styles.css"],
    };
  }

  if (normalized.includes("oscuro") || normalized.includes("dark")) {
    next.files["styles.css"] += "\n/* Agent edit: dark workshop */\n:root { color-scheme: dark; --ds-canvas: #191a17; --ds-surface: #252620; --ds-subtle: #2e2f28; --ds-border: #44463b; --ds-line: #44463b; --ds-stroke: #8a8c7e; --ds-text: #eeeade; --ds-muted: #b7b3a6; --ds-field: #1f201b; }\n";
    return {
      project: next,
      message: "Proposed change to styles.css: a dark theme. Nothing is saved until you apply it.",
      changedFiles: ["styles.css"],
    };
  }

  if (normalized.includes("fecha") || normalized.includes("date")) {
    next.files["index.html"] = project.files["index.html"].replace(
      '<input id="task-input" placeholder="Add one small thing…" autocomplete="off" required>',
      '<input id="task-input" placeholder="Add one small thing…" autocomplete="off" required><input id="due-input" type="date" aria-label="Due date">',
    );
    next.files["styles.css"] += "\n.ui-composer { grid-template-columns: minmax(0, 1fr) 150px auto; } .ui-composer input[type=date] { border-left: 1px solid var(--ds-border); }";
    return {
      project: next,
      message: "Proposed change to index.html and styles.css: a due date field. Nothing is saved until you apply it. Storing the date needs a due_at column.",
      changedFiles: ["index.html", "styles.css"],
    };
  }

  return {
    project: next,
    message: "The demo can propose a blue accent, a dark theme, or a date field. Connect a model in Agent settings for anything else.",
    changedFiles: [],
  };
}
