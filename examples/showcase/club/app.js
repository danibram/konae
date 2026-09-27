// A members-only book club. The Deployment is private and Invitation only, so
// every visitor who reaches this script is a signed-in member of the app.
const $ = (selector) => document.querySelector(selector);
const esc = (value) => String(value ?? "").replace(/[&<>"']/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" }[c]));
// The path miniapp.files.url returns; a cover is drawn in a template, which cannot wait for it.
const fileUrl = (id) => `/_littlebox/files/${encodeURIComponent(id)}`;

const status = $("#status");
function say(message, tone = "info") {
  status.textContent = message;
  status.dataset.tone = tone;
  status.hidden = !message;
}

// Organizer actions check the role in their WHERE clause and declare a guard,
// so a refused call rejects with the action's own sentence.
async function mutate(name, input, done) {
  try {
    await miniapp.actions.call(name, input);
    say(done, "ok");
    await render();
  } catch (error) {
    say(error.message, "refused");
  }
}

let me;
async function boot() {
  const session = await miniapp.auth.session();
  $("#who-name").textContent = session.name;
  [me] = await miniapp.actions.call("me.get");
  if (!me) { $("#join").hidden = false; return; }
  $("#join").hidden = true;
  $("#club").hidden = false;
  $("#who-name").textContent = me.display;
  $("#who-role").textContent = me.role;
  $("#who-role").dataset.role = me.role;
  await render();
}

function cover(book, size) {
  if (!book.cover_file_id) return `<div class="cover empty ${size}" aria-hidden="true">${esc(book.title.slice(0, 1))}</div>`;
  return `<img class="cover ${size}" src="${fileUrl(book.cover_file_id)}" alt="Cover of ${esc(book.title)}" loading="lazy">`;
}

async function render() {
  const [books, notes, members] = await Promise.all([
    miniapp.actions.call("books.list"),
    miniapp.actions.call("notes.mine"),
    miniapp.actions.call("members.list"),
  ]);
  const current = books.find((book) => book.status === "current");
  $("#current").innerHTML = current
    ? `<div class="feature">${cover(current, "large")}<div><p class="title">${esc(current.title)}</p><p class="muted">${esc(current.author)}</p>
        <p class="meeting">Meeting: <strong>${esc(current.meets_on)}</strong></p>
        <div class="actions"><button type="button" class="secondary" data-action="books.finish" data-id="${current.id}">Mark finished</button></div></div></div>`
    : `<p class="muted">No book is scheduled. An organizer picks one from the shortlist.</p>`;

  const shortlist = books.filter((book) => book.status !== "current");
  $("#shortlist").innerHTML = shortlist.map((book) => `<li class="book ${book.status}">
      ${cover(book, "small")}
      <div class="book-body">
        <p class="title">${esc(book.title)}</p>
        <p class="muted">${esc(book.author)} · proposed by ${esc(book.proposer ?? "a former member")}${book.status === "finished" ? " · finished" : ""}</p>
        <div class="actions">
          ${book.status === "proposed" ? `<button type="button" class="secondary" data-action="books.schedule" data-id="${book.id}">Read next</button>` : ""}
          <button type="button" class="ghost danger" data-action="books.remove" data-id="${book.id}">Remove</button>
          <span class="hint">Organizers only</span>
        </div>
      </div></li>`).join("") || `<li class="muted">Nothing proposed yet.</li>`;

  $("#note-form select").innerHTML = books.filter((book) => book.status !== "finished")
    .map((book) => `<option value="${book.id}">${esc(book.title)}</option>`).join("");
  $("#note-form button").disabled = !books.some((book) => book.status !== "finished");

  $("#notes").innerHTML = notes.map((note) => `<li>
      <details data-file="${esc(note.file_id)}"><summary><strong>${esc(note.book)}</strong> <span class="muted">${new Date(note.created_at).toLocaleDateString()}</span></summary><pre class="note-text">Opening…</pre></details>
    </li>`).join("") || `<li class="muted">No notes yet.</li>`;

  $("#members").innerHTML = members.map((member) => `<li><span>${esc(member.display)}</span><span class="badge" data-role="${esc(member.role)}">${esc(member.role)}</span></li>`).join("");
}

// A private note is read back through the file read path with the session
// cookie. The Kernel answers 404 to anyone but its uploader.
document.addEventListener("toggle", async (event) => {
  const details = event.target;
  if (!(details instanceof HTMLDetailsElement) || !details.open || details.dataset.loaded) return;
  const pre = details.querySelector("pre");
  const response = await fetch(await miniapp.files.url(details.dataset.file));
  pre.textContent = response.ok ? await response.text() : `This note could not be opened (${response.status}).`;
  details.dataset.loaded = "1";
}, true);

document.addEventListener("click", (event) => {
  const button = event.target.closest("button[data-action]");
  if (!button) return;
  const id = Number(button.dataset.id);
  if (button.dataset.action === "books.schedule") {
    const meetsOn = new Date(Date.now() + 14 * 86_400_000).toISOString().slice(0, 10);
    mutate("books.schedule", { id, meetsOn }, `Scheduled for ${meetsOn}.`);
  } else if (button.dataset.action === "books.finish") {
    mutate("books.finish", { id }, "Marked as finished.");
  } else if (button.dataset.action === "books.remove") {
    mutate("books.remove", { id }, "Removed.");
  }
});

$("#join-form").addEventListener("submit", async (event) => {
  event.preventDefault();
  const display = new FormData(event.target).get("display").trim();
  try { await miniapp.actions.call("me.join", { display }); await boot(); say(`Welcome, ${display}.`, "ok"); }
  catch (error) { say(error.message, "refused"); }
});

$("#propose-form").addEventListener("submit", async (event) => {
  event.preventDefault();
  const form = event.target;
  const data = new FormData(form);
  const file = data.get("cover");
  try {
    let coverFileId = null;
    // A cover is public inside this private app: every signed-in member reads it.
    if (file instanceof File && file.size > 0) coverFileId = (await miniapp.files.upload(file, { visibility: "public" })).id;
    const result = await miniapp.actions.call("books.propose", { title: data.get("title").trim(), author: data.get("author").trim(), coverFileId });
    if (result.changes === 0) { say("Join the club before proposing a book.", "refused"); return; }
    form.reset();
    say("Proposed.", "ok");
    await render();
  } catch (error) { say(error.message, "refused"); }
});

$("#note-form").addEventListener("submit", async (event) => {
  event.preventDefault();
  const form = event.target;
  const data = new FormData(form);
  const bookId = Number(data.get("book"));
  const title = form.querySelector("select").selectedOptions[0]?.textContent ?? "book";
  const name = `note-${title.toLowerCase().replace(/[^a-z0-9]+/g, "-").slice(0, 60)}.txt`;
  try {
    // Private is the default: only this member can list or read the file.
    const stored = await miniapp.files.upload(new File([data.get("body")], name, { type: "text/plain" }));
    await miniapp.actions.call("notes.add", { bookId, fileId: stored.id, name });
    form.reset();
    say("Note saved. Only you can open it.", "ok");
    await render();
  } catch (error) { say(error.message, "refused"); }
});

$("#sign-out").addEventListener("click", async () => {
  await miniapp.auth.signOut();
  location.reload();
});

boot().catch((error) => say(error.message, "refused"));
