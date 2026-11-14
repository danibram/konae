import type { RuntimeActionDeclaration } from "../deployments/manifest";
import type { MiniAppProject } from "./types";
import { appStyles, escapeHtmlSource } from "./templates/shared";

const privateNotesSchema = `CREATE TABLE IF NOT EXISTS notes (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  owner_id TEXT NOT NULL,
  title TEXT NOT NULL,
  created_at INTEGER NOT NULL
);`;

const privateNotesActions = {
  version: 1,
  actions: {
    "notes.list": {
      kind: "query",
      access: "authenticated",
      statement: "SELECT id, title, created_at FROM notes WHERE owner_id = ? ORDER BY created_at DESC LIMIT 100",
      bindings: [{ source: "actor", field: "id" }],
    },
    "notes.create": {
      kind: "mutation",
      access: "authenticated",
      statement: "INSERT INTO notes (owner_id, title, created_at) VALUES (?, ?, ?)",
      bindings: [
        { source: "actor", field: "id" },
        { source: "input", key: "title", type: "string", maxLength: 500 },
        { source: "kernel", field: "now" },
      ],
    },
    "notes.delete": {
      kind: "mutation",
      access: "authenticated",
      statement: "DELETE FROM notes WHERE id = ? AND owner_id = ?",
      bindings: [
        { source: "input", key: "id", type: "number" },
        { source: "actor", field: "id" },
      ],
    },
  },
};

const privateNotesDeclarations: RuntimeActionDeclaration[] = [
  { name: "notes.list", kind: "query", access: "authenticated" },
  { name: "notes.create", kind: "mutation", access: "authenticated" },
  { name: "notes.delete", kind: "mutation", access: "authenticated" },
];

const privateNotesIndex = `
<title>Private notes</title>
<main class="ui-page" data-purpose="Private notes. Each person signs in and sees only their own.">
  <section id="landing" class="landing" aria-labelledby="landing-title">
    <p class="ui-eyebrow">A quiet place to keep things</p>
    <h1 id="landing-title" class="ui-title">Your notes,<br><em>your space.</em></h1>
    <p class="ui-lede">Sign in to keep a private list that belongs to you.</p>
    <p id="landing-status" class="ui-status" role="status" aria-live="polite"></p>
    <button id="sign-in" class="ui-button ui-button-primary" disabled>Sign in</button>
  </section>

  <section id="notes-app" hidden aria-labelledby="notes-title">
    <header class="ui-page-header">
      <div>
        <p class="ui-eyebrow">Private notes</p>
        <h1 id="notes-title" class="ui-title">A place to<br><em>remember.</em></h1>
      </div>
      <div class="ui-identity">
        <span id="identity"></span>
        <button id="sign-out" class="ui-button ui-button-sm">Sign out</button>
      </div>
    </header>

    <div class="ui-composer">
      <input id="note-title" name="title" maxlength="500" placeholder="Keep a thought…" autocomplete="off" required>
      <button id="add-note" class="ui-button ui-button-primary" type="button">Save</button>
    </div>
    <p id="status" class="ui-status" role="status" aria-live="polite"></p>
    <div class="ui-section-head"><span>Your notes</span><span id="count">0 notes</span></div>
    <div id="notes" class="ui-list"></div>
  </section>
</main>
`;

const privateNotesStyles = appStyles(`.landing { min-height: 72vh; display: flex; flex-direction: column; align-items: flex-start; justify-content: center; }
.landing .ui-title { margin-bottom: 16px; }`);

const privateNotesApp = `
const landing = document.querySelector('#landing');
const app = document.querySelector('#notes-app');
const identity = document.querySelector('#identity');
const list = document.querySelector('#notes');
const count = document.querySelector('#count');
const status = document.querySelector('#status');
const landingStatus = document.querySelector('#landing-status');
const titleInput = document.querySelector('#note-title');

${escapeHtmlSource}

function showError(error) {
  const message = error && error.message ? error.message : 'Could not complete that request.';
  const line = app.hidden ? landingStatus : status;
  line.textContent = message;
  line.dataset.tone = 'danger';
}

async function loadNotes() {
  const notes = await miniapp.actions.call('notes.list');
  count.textContent = notes.length + (notes.length === 1 ? ' note' : ' notes');
  list.innerHTML = notes.length ? notes.map(note =>
    '<article class="ui-list-item note">' +
      '<div class="ui-list-main"><p>' + escapeHtml(note.title) + '</p><small>' + new Date(note.created_at).toLocaleString() + '</small></div>' +
      '<button type="button" class="ui-button ui-button-danger ui-button-sm" data-id="' + Number(note.id) + '">Remove</button>' +
    '</article>'
  ).join('') : '<p class="ui-list-empty">No notes yet. Start with one thought.</p>';
}

async function showCurrentSession() {
  const session = await miniapp.auth.session();
  const signedIn = session.role !== 'guest' && session.role !== 'anonymous';
  landing.hidden = signedIn;
  app.hidden = !signedIn;
  if (signedIn) {
    identity.textContent = session.name;
    await loadNotes();
  }
}

const signInButton = document.querySelector('#sign-in');
signInButton.addEventListener('click', async () => {
  try {
    status.textContent = '';
    await miniapp.auth.signIn();
    await showCurrentSession();
  } catch (error) { showError(error); }
});
// The button renders before this module runs; it stays disabled until a click can do something.
signInButton.disabled = false;

document.querySelector('#sign-out').addEventListener('click', async () => {
  try {
    await miniapp.auth.signOut();
    await showCurrentSession();
  } catch (error) { showError(error); }
});

async function addNote() {
  const title = titleInput.value.trim();
  if (!title) return;
  try {
    status.textContent = '';
    await miniapp.actions.call('notes.create', { title });
    titleInput.value = '';
    await loadNotes();
  } catch (error) { showError(error); }
}

document.querySelector('#add-note').addEventListener('click', addNote);
titleInput.addEventListener('keydown', event => {
  if (event.key === 'Enter') {
    event.preventDefault();
    void addNote();
  }
});

list.addEventListener('click', async event => {
  const button = event.target.closest('button[data-id]');
  if (!button) return;
  try {
    status.textContent = '';
    await miniapp.actions.call('notes.delete', { id: Number(button.dataset.id) });
    await loadNotes();
  } catch (error) { showError(error); }
});

try { await showCurrentSession(); } catch (error) { showError(error); }
`;

export function withPrivateNotes(project: MiniAppProject): MiniAppProject {
  const manifest = {
    ...project.manifest,
    runtime: {
      ...project.manifest.runtime,
      database: {
        ...project.manifest.runtime.database,
        actions: privateNotesDeclarations.map((action) => ({ ...action })),
      },
      auth: { mode: "optional" as const },
    },
  };

  return {
    ...project,
    directories: [...project.directories],
    files: {
      ...project.files,
      "index.html": privateNotesIndex,
      "styles.css": privateNotesStyles,
      "app.js": privateNotesApp,
      "schema.sql": privateNotesSchema,
      "actions.json": JSON.stringify(privateNotesActions, null, 2),
    },
    manifest,
    updatedAt: Date.now(),
  };
}
