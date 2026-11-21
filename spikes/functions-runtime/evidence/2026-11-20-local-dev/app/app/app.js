
const landing = document.querySelector('#landing');
const app = document.querySelector('#notes-app');
const identity = document.querySelector('#identity');
const list = document.querySelector('#notes');
const count = document.querySelector('#count');
const status = document.querySelector('#status');
const landingStatus = document.querySelector('#landing-status');
const titleInput = document.querySelector('#note-title');

function escapeHtml(value) {
  const entities = { '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;' };
  return String(value ?? '').replace(/[&<>"']/g, (char) => entities[char]);
}

function showError(error) {
  const message = error && error.message ? error.message : 'Could not complete that request.';
  const line = app.hidden ? landingStatus : status;
  line.textContent = message;
  line.dataset.tone = 'danger';
}

async function loadNotes() {
  const response = await fetch('/api/notes/', { credentials: 'same-origin' });
  const result = await response.json();
  if (!response.ok) throw new Error(result.error || 'Could not load notes.');
  const notes = result.notes;
  count.textContent = notes.length + (notes.length === 1 ? ' note' : ' notes') + ' · functions ' + result.version;
  list.innerHTML = notes.length ? notes.map(note =>
    '<article class="ui-list-item note">' +
      '<div class="ui-list-main"><p>' + escapeHtml(note.title) + '</p><small>' + new Date(note.created_at).toLocaleString() + '</small></div>' +
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
    const response = await fetch('/api/notes/', {
      method: 'POST',
      credentials: 'same-origin',
      headers: { 'content-type': 'application/json', 'idempotency-key': crypto.randomUUID() },
      body: JSON.stringify({ title }),
    });
    const result = await response.json();
    if (!response.ok) throw new Error(result.error || 'Could not save the note.');
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

try { await showCurrentSession(); } catch (error) { showError(error); }
