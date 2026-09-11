import type { RuntimeActionDeclaration } from "../deployments/manifest";
import type { MiniAppProject } from "./types";

export const privateNotesDatabaseModelSource = `import { integer, sqliteTable, text } from "drizzle-orm/sqlite-core";

export const notes = sqliteTable("notes", {
  id: integer("id").primaryKey({ autoIncrement: true }),
  ownerId: text("owner_id").notNull(),
  title: text("title").notNull(),
  createdAt: integer("created_at").notNull(),
});
`;

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
<main class="notes-shell">
  <section id="landing" class="landing" aria-labelledby="landing-title">
    <p class="eyebrow">A QUIET PLACE TO KEEP THINGS</p>
    <h1 id="landing-title">Your notes,<br><em>your space.</em></h1>
    <p class="intro">Sign in to keep a private list that belongs to you.</p>
    <p id="landing-status" class="status" role="status" aria-live="polite"></p>
    <button id="sign-in" class="primary" disabled>Sign in</button>
  </section>

  <section id="notes-app" class="notes-app" hidden aria-labelledby="notes-title">
    <header>
      <div>
        <p class="eyebrow">PRIVATE NOTES</p>
        <h1 id="notes-title">A place to<br><em>remember.</em></h1>
      </div>
      <div class="account">
        <span id="identity"></span>
        <button id="sign-out" class="quiet">Sign out</button>
      </div>
    </header>

    <div class="composer">
      <input id="note-title" name="title" maxlength="500" placeholder="Keep a thought…" autocomplete="off" required>
      <button id="add-note" class="primary" type="button">Save</button>
    </div>
    <p id="status" class="status" role="status" aria-live="polite"></p>
    <div class="section-label"><span>YOUR NOTES</span><span id="count">0 notes</span></div>
    <div id="notes" class="notes-list"></div>
  </section>
</main>
`;

const privateNotesStyles = `
@import url('https://fonts.googleapis.com/css2?family=DM+Serif+Display:ital@0;1&family=Manrope:wght@500;600;700&display=swap');
:root { color-scheme: light dark; font-family: Manrope, sans-serif; background: #f3efe5; color: #24251f; }
* { box-sizing: border-box; }
body { margin: 0; min-height: 100vh; background: radial-gradient(circle at 90% 8%, #d7e6b7 0 18%, transparent 38%), #f3efe5; }
.notes-shell { width: min(680px, calc(100% - 40px)); margin: 0 auto; padding: 64px 0; }
.landing { min-height: 72vh; display: flex; flex-direction: column; align-items: flex-start; justify-content: center; }
.eyebrow, .section-label { font-size: 11px; letter-spacing: .18em; font-weight: 700; }
h1 { font-family: 'DM Serif Display', serif; font-size: clamp(32px, 7vw, 56px); line-height: .9; letter-spacing: -.045em; margin: 8px 0 16px; font-weight: 400; }
h1 em { color: #e65337; }
.intro { max-width: 350px; color: #77756c; line-height: 1.7; margin: 0 0 24px; }
.primary { min-height: 46px; border: 1px solid #24251f; padding: 10px 18px; background: #e65337; color: white; font: inherit; font-weight: 700; cursor: pointer; box-shadow: 4px 4px 0 #24251f; }
.primary:disabled { opacity: .55; cursor: progress; }
.notes-app header { display: flex; justify-content: space-between; align-items: flex-start; gap: 24px; }
.account { display: flex; align-items: center; gap: 10px; font-size: 12px; font-weight: 700; }
.quiet { border: 1px solid #24251f; padding: 8px 11px; background: #fffdf8; font: inherit; font-size: 11px; cursor: pointer; }
.composer { background: #fffdf8; border: 1px solid #24251f; display: grid; grid-template-columns: 1fr auto; gap: 12px; padding: 8px; box-shadow: 6px 6px 0 #24251f; margin: 0 0 20px; }
.composer input { min-width: 0; border: 0; background: transparent; padding: 12px; font: inherit; outline: none; }
.status { min-height: 20px; color: #9d3929; font-size: 12px; }
.section-label { display: flex; justify-content: space-between; border-bottom: 1px solid #969487; padding: 18px 0 10px; }
.notes-list { display: grid; }
.note { display: flex; justify-content: space-between; align-items: center; gap: 18px; padding: 18px 4px; border-bottom: 1px solid #c9c5b9; }
.note p { margin: 0; overflow-wrap: anywhere; line-height: 1.5; }
.note small { display: block; margin-top: 5px; color: #77756c; }
.note button { flex: 0 0 auto; border: 0; background: transparent; color: #9d3929; font: inherit; font-size: 12px; text-decoration: underline; cursor: pointer; }
.empty { font-family: 'DM Serif Display', serif; font-size: 28px; color: #77756c; padding: 48px 4px; }
[hidden] { display: none !important; }
@media (max-width: 520px) { .notes-shell { padding-top: 36px; } .notes-app header { display: block; } .account { justify-content: space-between; margin: -10px 0 30px; } }
@media (prefers-color-scheme: dark) {
  :root { background: #1c1b17; color: #f3efe5; }
  body { background: #1c1b17; }
  .composer, .quiet { background: #2a2822; color: inherit; border-color: #f3efe5; }
  .composer { box-shadow: 6px 6px 0 #000; }
  .note, .section-label { border-color: #5c594f; }
  .intro, .note small, .empty { color: #b7b3a6; }
}
`;

const privateNotesApp = `
const landing = document.querySelector('#landing');
const app = document.querySelector('#notes-app');
const identity = document.querySelector('#identity');
const list = document.querySelector('#notes');
const count = document.querySelector('#count');
const status = document.querySelector('#status');
const landingStatus = document.querySelector('#landing-status');
const titleInput = document.querySelector('#note-title');

function escapeHtml(value) {
  const element = document.createElement('span');
  element.textContent = value;
  return element.innerHTML;
}

function showError(error) {
  const message = error && error.message ? error.message : 'Could not complete that request.';
  (app.hidden ? landingStatus : status).textContent = message;
}

async function loadNotes() {
  const notes = await miniapp.actions.call('notes.list');
  count.textContent = notes.length + (notes.length === 1 ? ' note' : ' notes');
  list.innerHTML = notes.length ? notes.map(note =>
    '<article class="note">' +
      '<div><p>' + escapeHtml(note.title) + '</p><small>' + new Date(note.created_at).toLocaleString() + '</small></div>' +
      '<button type="button" data-id="' + Number(note.id) + '">Remove</button>' +
    '</article>'
  ).join('') : '<div class="empty">No notes yet. Start with one thought.</div>';
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
    databaseModel: {
      source: privateNotesDatabaseModelSource,
      acceptedSource: privateNotesDatabaseModelSource,
      revision: "baseline",
    },
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
