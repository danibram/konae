// Room board. Visitors read which hours are taken; signed-in staff book,
// cancel and see who holds a room. Every read and write is a declared action.
const $ = (selector) => document.querySelector(selector);
const esc = (value) => String(value ?? "").replace(/[&<>"]/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;" }[c]));
const HOURS = [8, 9, 10, 11, 12, 13, 14, 15, 16, 17];
const label = (hour) => `${String(hour).padStart(2, "0")}:00`;
const localDay = (date = new Date()) => `${date.getFullYear()}-${String(date.getMonth() + 1).padStart(2, "0")}-${String(date.getDate()).padStart(2, "0")}`;
const shiftDay = (day, delta) => { const d = new Date(`${day}T12:00:00`); d.setDate(d.getDate() + delta); return localDay(d); };

const session = await miniapp.auth.session();
const staff = session.role !== "anonymous";
let day = localDay();
let rooms = [];
let picked = null;

// Every status line reads a failure the same way. A refusal's message is the
// sentence to show; the rate limit answers with a code and an unreachable
// Kernel with a browser error, so those two get a sentence here.
function reason(error) {
  if (error?.message === "rate-limited") return "Too many requests just now. Wait a few seconds and try again.";
  if (error instanceof TypeError && /fetch|network|load failed/i.test(error.message)) return "The board could not reach its server. Check the connection and try again.";
  return error?.message ?? String(error);
}

function say(text, tone = "") {
  const status = $("#status");
  status.textContent = text;
  status.className = `status ${tone}`;
}

async function renderWho() {
  if (!staff) {
    $("#who").innerHTML = `<button type="button" class="btn primary" id="signin">Staff sign in</button>`;
    $("#signin").addEventListener("click", () => miniapp.auth.signIn());
    return;
  }
  let [me] = await miniapp.actions.call("staff.me");
  if (!me) {
    await miniapp.actions.call("staff.name", { display: session.name.split("@")[0].slice(0, 60) });
    [me] = await miniapp.actions.call("staff.me");
  }
  $("#who").innerHTML = `<form id="rename"><label class="field">Shown to colleagues as<input id="display" maxlength="60" required value="${esc(me.display)}"></label><button class="btn secondary" type="submit">Save</button></form><button type="button" class="btn ghost" id="signout">Sign out</button>`;
  $("#rename").addEventListener("submit", async (event) => {
    event.preventDefault();
    const button = event.target.querySelector("button[type=submit]");
    button.disabled = true;
    try { await run("staff.name", { display: $("#display").value.trim() }, "Name saved."); }
    finally { button.disabled = false; }
  });
  $("#signout").addEventListener("click", async () => { await miniapp.auth.signOut(); location.reload(); });
  $("#lede").textContent = `Signed in as ${session.name}. Pick a free hour to book it.`;
}

async function renderGrid() {
  const shown = day;
  const rows = staff
    ? await miniapp.actions.call("bookings.day", { day: shown })
    : await miniapp.actions.call("availability.day", { day: shown });
  // A slower answer for a day the person already left is dropped.
  if (shown !== day) return;
  const held = new Map(rows.map((row) => [`${row.room}/${row.hour}`, row]));
  const past = shown < localDay();
  const head = `<thead><tr><th scope="col">Room</th>${HOURS.map((h) => `<th scope="col">${label(h)}</th>`).join("")}</tr></thead>`;
  const body = rooms.map((room) => `<tr><th scope="row" class="room">${esc(room.name)}<small>${room.seats} seats</small></th>${HOURS.map((hour) => {
    const row = held.get(`${room.id}/${hour}`);
    const where = `${esc(room.name)} at ${label(hour)}`;
    if (row && staff) {
      return `<td><div class="cell ${row.mine ? "mine" : "held"}" title="${esc(row.title)}, ${esc(row.booked_by)}"><strong>${esc(row.title)}</strong>${row.mine ? "You" : esc(row.booked_by)}${row.checked_in_at ? ", checked in" : ""}</div></td>`;
    }
    if (row) return `<td><div class="cell held" aria-label="${where}, booked">Booked</div></td>`;
    if (!staff || past) return `<td><div class="cell" aria-label="${where}, free">Free</div></td>`;
    const on = picked && picked.room === room.id && picked.hour === hour ? " picked" : "";
    return `<td><button type="button" class="cell${on}" data-room="${esc(room.id)}" data-hour="${hour}" aria-label="Book ${where}">Free</button></td>`;
  }).join("")}</tr>`).join("");
  $("#grid").innerHTML = head + `<tbody>${body || `<tr><td colspan="11" class="muted">No rooms yet. The owner adds them.</td></tr>`}</tbody>`;
  $("#grid").dataset.day = shown;
}

async function renderStaff() {
  if (!staff) return;
  $("#staff").hidden = false;
  const [mine, recent] = await Promise.all([
    miniapp.actions.call("bookings.mine", { from: localDay() }),
    miniapp.actions.call("bookings.recent"),
  ]);
  const roomName = (id) => rooms.find((room) => room.id === id)?.name ?? id;
  const span = (row) => `${row.day} ${label(row.start)} to ${label(row.start + row.hours)}`;
  $("#mine").innerHTML = mine.map((row) => `<li><span>${esc(row.title)}<span class="meta">${esc(roomName(row.room))}, ${esc(span(row))}</span></span><button type="button" class="btn danger" data-cancel="${esc(row.ref)}">Cancel</button></li>`).join("")
    || `<li class="muted">No upcoming bookings.</li>`;
  $("#recent").innerHTML = recent.map((row) => `<li><span>${esc(row.title)}<span class="meta">${esc(row.booked_by)}, ${esc(roomName(row.room))}, ${esc(span(row))}</span></span>${row.cancelled_at ? `<span class="badge cancel">Cancelled</span>` : `<span class="badge new">Booked</span>`}</li>`).join("")
    || `<li class="muted">Nothing booked yet.</li>`;
}

async function render() {
  $("#day").value = day;
  rooms = await miniapp.actions.call("rooms.list");
  await Promise.all([renderGrid(), renderStaff()]);
}

async function run(name, input, done) {
  try {
    const result = await miniapp.actions.call(name, input);
    await render();
    if (done) say(done, "ok");
    return result;
  } catch (error) {
    say(reason(error), "error");
    return null;
  }
}

function openForm(room, hour) {
  picked = { room, hour };
  const name = rooms.find((r) => r.id === room)?.name ?? room;
  $("#book-title").textContent = `Book ${name} on ${day} from ${label(hour)}`;
  $("#book").hidden = false;
  $("#title").focus();
  renderGrid();
}
function closeForm() {
  picked = null;
  $("#book").hidden = true;
  $("#book").reset();
  renderGrid();
}

$("#grid").addEventListener("click", (event) => {
  const cell = event.target.closest("button[data-room]");
  if (cell) openForm(cell.dataset.room, Number(cell.dataset.hour));
});
$("#dismiss").addEventListener("click", closeForm);
$("#book").addEventListener("submit", async (event) => {
  event.preventDefault();
  if (!picked) return;
  const button = $("#confirm");
  button.disabled = true;
  const hours = Number($("#hours").value);
  try {
    const result = await miniapp.actions.call("bookings.create", { room: picked.room, day, start: picked.hour, hours, title: $("#title").value.trim() });
    say(`Booked ${result.changes} hour${result.changes > 1 ? "s" : ""}.`, "ok");
    closeForm();
    await render();
  } catch (error) {
    // A booking that inserts nothing is refused with the action's guard, which names the rules.
    say(reason(error), "error");
    await render().catch(() => {});
  } finally {
    button.disabled = false;
  }
});
$("#mine").addEventListener("click", async (event) => {
  const button = event.target.closest("button[data-cancel]");
  if (!button) return;
  button.disabled = true;
  // A cancel that changes nothing is refused with the action's guard, which run() shows.
  await run("bookings.cancel", { ref: button.dataset.cancel }, "Booking cancelled. The hours are free again.");
});
const go = (next) => {
  day = next; picked = null; $("#book").hidden = true; say("");
  render().catch((error) => say(reason(error), "error"));
};
$("#prev").addEventListener("click", () => go(shiftDay(day, -1)));
$("#next").addEventListener("click", () => go(shiftDay(day, 1)));
$("#today").addEventListener("click", () => go(localDay()));
$("#day").addEventListener("change", (event) => { if (event.target.value) go(event.target.value); });

// Nothing pushes changes to the page, so the open board asks again.
setInterval(() => { if (document.visibilityState === "visible" && !picked) render().catch(() => {}); }, 30_000);

try {
  await renderWho();
  await render();
} catch (error) {
  say(reason(error), "error");
}
document.body.dataset.ready = "true";
