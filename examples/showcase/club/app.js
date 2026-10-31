// A members-only book club. The Deployment is private and Invitation only, so
// every visitor who reaches this script is a signed-in member of the app.
const $ = (selector) => document.querySelector(selector);
const esc = (value) => String(value ?? "").replace(/[&<>"']/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" }[c]));
// The path miniapp.files.url returns; a cover is drawn in a template, which cannot wait for it.
const fileUrl = (id) => `/_konae/files/${encodeURIComponent(id)}`;

const status = $("#status");
// Every status line reads a failure the same way. A refusal's message is the
// sentence to show; the rate limit answers with a code and an unreachable
// Kernel with a browser error, so those two get a sentence here.
function reason(error) {
  if (error?.message === "rate-limited") return "Too many requests just now. Wait a few seconds and try again.";
  if (error instanceof TypeError && /fetch|network|load failed/i.test(error.message)) return "The club could not reach its server. Check the connection and try again.";
  return error?.message ?? String(error);
}
const localDay = (date) => `${date.getFullYear()}-${String(date.getMonth() + 1).padStart(2, "0")}-${String(date.getDate()).padStart(2, "0")}`;

// A form's submit button stays disabled until its call answers, so a second
// press cannot store the same book or note twice.
async function busy(form, work) {
  const button = form.querySelector("button[type=submit]");
  button.disabled = true;
  try { await work(); } finally { button.disabled = false; }
}

// A file uploaded for an action the club then refused is deleted, so it does
// not stay behind counting against the uploader's quota.
async function attach(file, options, use) {
  const stored = await miniapp.files.upload(file, options);
  try { return await use(stored); }
  catch (error) { await miniapp.files.delete(stored.id).catch(() => {}); throw error; }
}
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
    say(reason(error), "refused");
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
  try {
    const response = await fetch(await miniapp.files.url(details.dataset.file));
    pre.textContent = response.ok ? await response.text() : `This note could not be opened (${response.status}).`;
    if (response.ok) details.dataset.loaded = "1";
  } catch (error) {
    pre.textContent = `This note could not be opened. ${reason(error)}`;
  }
}, true);

document.addEventListener("click", (event) => {
  const button = event.target.closest("button[data-action]");
  if (!button) return;
  const id = Number(button.dataset.id);
  if (button.dataset.action === "books.schedule") {
    // Two weeks from today on the organizer's own calendar, not the UTC date.
    const meetsOn = localDay(new Date(new Date().setDate(new Date().getDate() + 14)));
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
  await busy(event.target, async () => {
    try { await miniapp.actions.call("me.join", { display }); await boot(); say(`Welcome, ${display}.`, "ok"); }
    catch (error) { say(reason(error), "refused"); }
  });
});

$("#propose-form").addEventListener("submit", async (event) => {
  event.preventDefault();
  const form = event.target;
  const data = new FormData(form);
  const file = data.get("cover");
  const book = { title: data.get("title").trim(), author: data.get("author").trim() };
  await busy(form, async () => {
    try {
      // A cover is public inside this private app: every signed-in member reads it.
      // books.propose declares a guard, so a refused proposal rejects with its sentence.
      if (file instanceof File && file.size > 0) {
        await attach(file, { visibility: "public" }, (cover) => miniapp.actions.call("books.propose", { ...book, coverFileId: cover.id }));
      } else {
        await miniapp.actions.call("books.propose", { ...book, coverFileId: null });
      }
      form.reset();
      say("Proposed.", "ok");
      await render();
    } catch (error) { say(reason(error), "refused"); }
  });
});

$("#note-form").addEventListener("submit", async (event) => {
  event.preventDefault();
  const form = event.target;
  const data = new FormData(form);
  const bookId = Number(data.get("book"));
  const title = form.querySelector("select").selectedOptions[0]?.textContent ?? "book";
  const name = `note-${title.toLowerCase().replace(/[^a-z0-9]+/g, "-").slice(0, 60)}.txt`;
  await busy(form, async () => {
    try {
      // Private is the default: only this member can list or read the file.
      await attach(new File([data.get("body")], name, { type: "text/plain" }), undefined,
        (stored) => miniapp.actions.call("notes.add", { bookId, fileId: stored.id, name }));
      form.reset();
      say("Note saved. Only you can open it.", "ok");
      await render();
    } catch (error) { say(reason(error), "refused"); }
  });
});

$("#sign-out").addEventListener("click", async () => {
  await miniapp.auth.signOut();
  location.reload();
});

boot().catch((error) => say(reason(error), "refused"));
