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
      message: "He cambiado el acento a azul tinta y he ajustado el halo del fondo para mantener el contraste.",
      changedFiles: ["styles.css"],
    };
  }

  if (normalized.includes("oscuro") || normalized.includes("dark")) {
    next.files["styles.css"] += `\n\n/* Agent edit: dark workshop */\n:root { background: #191a17; color: #eeeade; }\nbody { background: radial-gradient(circle at 90% 8%, #33402c 0 18%, transparent 38%), #191a17; }\n.composer { background: #252620; box-shadow: 6px 6px 0 #e8e4d8; }\n.composer input { color: #eeeade; }\n.identity, .task button { border-color: #eeeade; }`;
    return {
      project: next,
      message: "He añadido un modo oscuro editorial sin tocar la estructura ni los datos.",
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
      message: "He añadido el control visual para fechas. En el siguiente paso conectaría la columna `due_at` con una migración segura.",
      changedFiles: ["index.html", "styles.css"],
    };
  }

  return {
    project: next,
    message: "He analizado el cambio. Este prototipo local ya entiende cambios de color azul, modo oscuro y añadir fechas. El siguiente adaptador conectará este mismo flujo a un modelo real.",
    changedFiles: [],
  };
}
