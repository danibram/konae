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
    next.files["styles.css"] = project.files["styles.css"]
      .replaceAll("#e65337", "#3157d5")
      .replaceAll("#d7e6b7", "#b9d7ef");
    return {
      project: next,
      message: "Proposed change to styles.css: the accent becomes ink blue. Nothing is saved until you apply it.",
      changedFiles: ["styles.css"],
    };
  }

  if (normalized.includes("oscuro") || normalized.includes("dark")) {
    next.files["styles.css"] += `\n\n/* Agent edit: dark workshop */\n:root { background: #191a17; color: #eeeade; }\nbody { background: radial-gradient(circle at 90% 8%, #33402c 0 18%, transparent 38%), #191a17; }\n.composer { background: #252620; box-shadow: 6px 6px 0 #e8e4d8; }\n.composer input { color: #eeeade; }\n.identity, .task button { border-color: #eeeade; }`;
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
    next.files["styles.css"] += "\n.composer { grid-template-columns: 1fr 150px 52px; } .composer input[type=date] { border-left: 1px solid #c9c5b9; }";
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
