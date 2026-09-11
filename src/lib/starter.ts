import type { MiniAppProject } from "./types";
import { createDefaultManifest } from "../deployments/manifest";

export const starterDatabaseModelSource = `import { integer, sqliteTable, text } from "drizzle-orm/sqlite-core";

export const tasks = sqliteTable("tasks", {
  id: integer("id").primaryKey({ autoIncrement: true }),
  title: text("title").notNull(),
  done: integer("done", { mode: "boolean" }).notNull().default(false),
  createdAt: integer("created_at").notNull(),
});
`;

export const starterProject: MiniAppProject = {
  id: "app_tiny_tasks",
  name: "Tiny Tasks",
  slug: "tiny-tasks",
  manifest: createDefaultManifest(),
  updatedAt: Date.now(),
  databaseModel: {
    source: starterDatabaseModelSource,
    acceptedSource: starterDatabaseModelSource,
    revision: "baseline",
  },
  directories: [],
  files: {
    "index.html": `
<main class="app-shell">
  <header>
    <div>
      <p class="eyebrow">MY QUIET LIST</p>
      <h1>Things worth<br><em>finishing.</em></h1>
    </div>
    <div id="identity" class="identity">guest</div>
  </header>

  <form id="task-form" class="composer">
    <input id="task-input" placeholder="Add one small thing…" autocomplete="off" required>
    <button id="add-task" type="button" aria-label="Add task">↗</button>
  </form>

  <section>
    <div class="section-label"><span>TODAY</span><span id="count">0 items</span></div>
    <div id="tasks" class="tasks"></div>
  </section>

  <section class="file-shelf">
    <div class="section-label"><span>PINBOARD</span><label for="file-input">+ ADD FILE</label></div>
    <input id="file-input" type="file" accept="image/*,.pdf,.txt" hidden>
    <div id="files" class="files"></div>
  </section>
</main>`,
    "styles.css": `
@import url('https://fonts.googleapis.com/css2?family=DM+Serif+Display:ital@0;1&family=Manrope:wght@500;600;700&display=swap');

:root { color-scheme: light; font-family: Manrope, sans-serif; background: #f3efe5; color: #24251f; }
* { box-sizing: border-box; }
body { margin: 0; min-height: 100vh; background: radial-gradient(circle at 90% 8%, #d7e6b7 0 18%, transparent 38%), #f3efe5; }
.app-shell { width: min(680px, calc(100% - 40px)); margin: 0 auto; padding: 64px 0; }
header { display: flex; justify-content: space-between; align-items: flex-start; gap: 24px; }
.eyebrow, .section-label { font-size: 11px; letter-spacing: .18em; font-weight: 700; }
h1 { font-family: 'DM Serif Display', serif; font-size: clamp(52px, 9vw, 84px); line-height: .84; letter-spacing: -.045em; margin: 14px 0 42px; font-weight: 400; }
h1 em { color: #e65337; }
.identity { border: 1px solid #24251f; border-radius: 999px; padding: 8px 12px; font-size: 11px; font-weight: 700; }
.composer { background: #fffdf8; border: 1px solid #24251f; display: grid; grid-template-columns: 1fr 52px; padding: 6px; box-shadow: 6px 6px 0 #24251f; margin-bottom: 52px; }
.composer input { border: 0; background: transparent; padding: 14px 16px; font: inherit; outline: none; }
.composer button { border: 0; background: #e65337; color: white; font-size: 25px; cursor: pointer; }
.section-label { display: flex; justify-content: space-between; border-bottom: 1px solid #969487; padding-bottom: 10px; }
.tasks { display: grid; }
.task { display: grid; grid-template-columns: 28px 1fr auto; gap: 12px; align-items: center; padding: 18px 4px; border-bottom: 1px solid #c9c5b9; }
.task button { width: 20px; height: 20px; border: 1px solid #24251f; border-radius: 50%; background: transparent; cursor: pointer; }
.task.done span { text-decoration: line-through; opacity: .42; }
.task small { color: #77756c; }
.empty { font-family: 'DM Serif Display', serif; font-size: 28px; color: #77756c; padding: 48px 4px; }
.file-shelf { margin-top: 54px; }
.section-label label { color: #e65337; cursor: pointer; }
.files { display: flex; gap: 10px; flex-wrap: wrap; padding-top: 15px; }
.file { width: 82px; height: 82px; display: grid; place-items: center; overflow: hidden; border: 1px solid #24251f; background: #fffdf8; font-size: 10px; text-align: center; }
.file img { width: 100%; height: 100%; object-fit: cover; }
@media (max-width: 520px) { .app-shell { padding-top: 36px; } .identity { display: none; } }
`,
    "app.js": `
const form = document.querySelector('#task-form');
const input = document.querySelector('#task-input');
const addButton = document.querySelector('#add-task');
const list = document.querySelector('#tasks');
const count = document.querySelector('#count');
const identity = document.querySelector('#identity');
const fileInput = document.querySelector('#file-input');
const fileShelf = document.querySelector('#files');

async function loadTasks() {
  const tasks = await miniapp.actions.call('tasks.list');
  count.textContent = tasks.length + (tasks.length === 1 ? ' item' : ' items');
  list.innerHTML = tasks.length ? tasks.map(task => \
    '<article class="task ' + (task.done ? 'done' : '') + '">' +
      '<button data-id="' + task.id + '" aria-label="Toggle task"></button>' +
      '<span>' + escapeHtml(task.title) + '</span>' +
      '<small>#' + task.id + '</small>' +
    '</article>'
  ).join('') : '<div class="empty">Nothing here. Lovely.</div>';
}

async function loadFiles() {
  const files = await miniapp.files.list();
  fileShelf.innerHTML = files.map(file =>
    '<div class="file" title="' + escapeHtml(file.name) + '">' +
    (file.type.startsWith('image/')
      ? '<img src="' + file.url + '" alt="' + escapeHtml(file.name) + '">'
      : escapeHtml(file.name)) +
    '</div>'
  ).join('');
}

function escapeHtml(value) {
  const node = document.createElement('span');
  node.textContent = value;
  return node.innerHTML;
}

async function addTask() {
  const title = input.value.trim();
  if (!title) return;
  await miniapp.actions.call('tasks.create', { title });
  input.value = '';
  await loadTasks();
}

form.addEventListener('submit', (event) => event.preventDefault());
addButton.addEventListener('click', addTask);
input.addEventListener('keydown', (event) => {
  if (event.key === 'Enter') {
    event.preventDefault();
    void addTask();
  }
});

fileInput.addEventListener('change', async () => {
  const file = fileInput.files[0];
  if (!file) return;
  await miniapp.files.upload(file);
  fileInput.value = '';
  await loadFiles();
});

list.addEventListener('click', async (event) => {
  const button = event.target.closest('button[data-id]');
  if (!button) return;
  await miniapp.actions.call('tasks.toggle', { id: Number(button.dataset.id) });
  await loadTasks();
});

const session = await miniapp.auth.session();
identity.textContent = session.name;
await Promise.all([loadTasks(), loadFiles()]);
`,
    "schema.sql": `
CREATE TABLE IF NOT EXISTS tasks (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  title TEXT NOT NULL,
  done INTEGER NOT NULL DEFAULT 0,
  created_at INTEGER NOT NULL
);
`,
    "actions.json": `{
  "version": 1,
  "actions": {
    "tasks.list": {
      "kind": "query",
      "access": "anonymous",
      "statement": "SELECT id, title, done, created_at FROM tasks ORDER BY created_at DESC LIMIT 100",
      "bindings": []
    },
    "tasks.create": {
      "kind": "mutation",
      "access": "anonymous",
      "statement": "INSERT INTO tasks (title, done, created_at) VALUES (?, 0, ?)",
      "bindings": [
        { "source": "input", "key": "title", "type": "string", "maxLength": 200 },
        { "source": "kernel", "field": "now" }
      ]
    },
    "tasks.toggle": {
      "kind": "mutation",
      "access": "anonymous",
      "statement": "UPDATE tasks SET done = CASE done WHEN 0 THEN 1 ELSE 0 END WHERE id = ?",
      "bindings": [
        { "source": "input", "key": "id", "type": "number" }
      ]
    }
  }
}
`,
  },
};

export function cloneStarter(): MiniAppProject {
  return {
    ...starterProject,
    directories: [...starterProject.directories],
    files: { ...starterProject.files },
    databaseModel: { ...starterProject.databaseModel },
    manifest: createDefaultManifest(),
    updatedAt: Date.now(),
  };
}
