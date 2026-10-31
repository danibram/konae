import type { MiniAppProject } from "./types";
import { createDefaultManifest } from "../deployments/manifest";
import { appStyles } from "./templates/shared";

/** The design system, then nothing: the starter needs no rules of its own. */
const starterStyles = appStyles();

export const starterProject: MiniAppProject = {
  id: "app_tiny_tasks",
  name: "Tiny Tasks",
  slug: "tiny-tasks",
  manifest: createDefaultManifest(),
  updatedAt: Date.now(),
  directories: [],
  files: {
    "index.html": `
<title>Tiny Tasks</title>
<main class="ui-page" data-purpose="A shared list. Anyone with the link can add to it.">
  <header class="ui-page-header">
    <div>
      <p class="ui-eyebrow">Shared list</p>
      <h1 class="ui-title">Tiny Tasks</h1>
    </div>
    <div id="identity" class="ui-identity">Guest</div>
  </header>

  <form id="task-form" class="ui-composer">
    <input id="task-input" placeholder="Add one small thing…" autocomplete="off" required>
    <button id="add-task" type="button" class="ui-button ui-button-primary" aria-label="Add task">Add</button>
  </form>

  <section>
    <div class="ui-section-head"><span>Today</span><span id="count"></span></div>
    <div id="tasks" class="ui-list"><p class="ui-list-empty">Nothing here yet. Anyone with this link sees the same list.</p></div>
    <p class="ui-list-note">The list stops at 100 rows.</p>
  </section>

  <section>
    <div class="ui-section-head"><span>Pinboard</span><label for="file-input" class="ui-button ui-button-sm">Add file</label></div>
    <input id="file-input" type="file" accept="image/*,.pdf,.txt" hidden>
    <div id="files" class="ui-tiles ui-tiles-small"></div>
  </section>
</main>`,
    "styles.css": starterStyles,
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
  list.innerHTML = tasks.length ? tasks.map(task =>
    '<article class="ui-list-item task ' + (task.done ? 'done' : '') + '"' + (task.done ? ' data-done' : '') + '>' +
      '<span class="ui-list-lead"><button class="ui-tick" data-id="' + task.id + '" aria-label="Toggle task" aria-pressed="' + (task.done ? 'true' : 'false') + '"></button></span>' +
      '<div class="ui-list-main"><p>' + escapeHtml(task.title) + '</p></div>' +
      '<div class="ui-list-end">#' + task.id + '</div>' +
    '</article>'
  ).join('') : '<p class="ui-list-empty">Nothing here yet. Anyone with this link sees the same list.</p>';
}

async function loadFiles() {
  const files = await miniapp.files.list();
  fileShelf.innerHTML = files.map(file =>
    '<figure class="ui-tile" title="' + escapeHtml(file.name) + '">' +
    (file.type.startsWith('image/')
      ? '<img src="' + file.url + '" alt="' + escapeHtml(file.name) + '">'
      : '<div class="ui-tile-media">' + escapeHtml(file.name) + '</div>') +
    '</figure>'
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
  // Preview sign-in stays on this page, so keep the selected File until it finishes.
  // A published app navigates to sign-in and requires selecting the file again.
  if ((await miniapp.auth.session()).role === 'anonymous') {
    await miniapp.auth.signIn();
    const session = await miniapp.auth.session();
    if (session.role === 'anonymous') {
      fileInput.value = '';
      return;
    }
    identity.textContent = session.name;
  }
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
identity.textContent = session.role === 'anonymous' ? 'Guest' : session.name;
// Tasks are declared anonymous. A file list shows this user's private files and any public files.
await loadTasks();
await loadFiles().catch(() => {});
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
    manifest: createDefaultManifest(),
    updatedAt: Date.now(),
  };
}

const droppedPreviewUpload = `if ((await miniapp.auth.session()).role === 'anonymous') {
    fileInput.value = '';
    miniapp.auth.signIn();
    return;
  }`;

const keptPreviewUpload = `if ((await miniapp.auth.session()).role === 'anonymous') {
    await miniapp.auth.signIn();
    const session = await miniapp.auth.session();
    if (session.role === 'anonymous') {
      fileInput.value = '';
      return;
    }
    identity.textContent = session.name;
  }`;

/** Rewrites the starter upload handler that cleared the File before sign-in finished. */
export function repairDroppedPreviewUpload(project: MiniAppProject): MiniAppProject {
  const script = project.files["app.js"];
  if (!script?.includes(droppedPreviewUpload)) return project;
  return {
    ...project,
    files: { ...project.files, "app.js": script.replace(droppedPreviewUpload, keptPreviewUpload) },
  };
}
