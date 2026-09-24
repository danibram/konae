// Habits: one person, two devices, one account. Every row belongs to the
// signed-in account; the Kernel binds the actor, never the page.
const $ = (selector) => document.querySelector(selector);
const pad = (n) => String(n).padStart(2, "0");
// The person's own calendar day, not the server's UTC day.
const localDay = (date) => `${date.getFullYear()}-${pad(date.getMonth() + 1)}-${pad(date.getDate())}`;
const shiftDay = (day, offset) => {
  const [y, m, d] = day.split("-").map(Number);
  return localDay(new Date(y, m - 1, d + offset));
};
const weekday = (day) => {
  const [y, m, d] = day.split("-").map(Number);
  return new Date(y, m - 1, d).toLocaleDateString("en", { weekday: "narrow" });
};
const device = matchMedia("(pointer: coarse)").matches ? "phone" : "computer";

const today = localDay(new Date());
const week = Array.from({ length: 7 }, (_, i) => shiftDay(today, i - 6));
$("#today").textContent = new Date().toLocaleDateString("en", { weekday: "long", month: "long", day: "numeric" });
$("#device").textContent = `Checking in from this ${device}.`;

function say(message, error = false) {
  const status = $("#status");
  status.textContent = message;
  status.classList.toggle("error", error);
}

async function load() {
  const [habits, checkins, streaks] = await Promise.all([
    miniapp.actions.call("habits.list"),
    miniapp.actions.call("checkins.since", { from: week[0] }),
    miniapp.actions.call("habits.streaks", { today }),
  ]);
  const done = new Set(checkins.map((row) => `${row.habit_id}:${row.day}`));
  const streakOf = new Map(streaks.map((row) => [row.habit_id, row]));
  const list = $("#habits");
  list.replaceChildren(...habits.map((habit) => render(habit, done, streakOf.get(habit.id))));
  $("#empty").hidden = habits.length > 0;
}

function render(habit, done, streak) {
  const item = document.createElement("li");
  item.className = "card habit";

  const head = document.createElement("div");
  head.className = "habit-head";
  const name = document.createElement("span");
  name.className = "habit-name";
  name.textContent = habit.name;
  const archive = document.createElement("button");
  archive.type = "button";
  archive.className = "ghost";
  archive.textContent = "Archive";
  archive.setAttribute("aria-label", `Archive ${habit.name}`);
  archive.addEventListener("click", () => act(() => miniapp.actions.call("habits.archive", { id: habit.id }), `Archived ${habit.name}.`));
  head.append(name, archive);

  const numbers = document.createElement("div");
  numbers.className = "streak";
  const current = streak?.current_streak ?? 0;
  numbers.append(
    stat(current, "day streak", current > 0),
    stat(streak?.best_streak ?? 0, "best"),
    stat(streak?.total_days ?? 0, "total"),
  );

  const days = document.createElement("div");
  days.className = "days";
  for (const day of week) {
    const checked = done.has(`${habit.id}:${day}`);
    const button = document.createElement("button");
    button.type = "button";
    button.className = `day${checked ? " done" : ""}${day === today ? " today" : ""}`;
    button.setAttribute("aria-pressed", String(checked));
    button.setAttribute("aria-label", `${habit.name} on ${day}${checked ? ", done" : ""}`);
    const label = document.createElement("span");
    label.textContent = day === today ? "Today" : weekday(day);
    const dot = document.createElement("span");
    dot.className = "dot";
    button.append(label, dot);
    button.addEventListener("click", () => act(
      () => checked
        ? miniapp.actions.call("checkins.remove", { habitId: habit.id, day })
        : miniapp.actions.call("checkins.add", { habitId: habit.id, day, device }),
      checked ? `Unchecked ${habit.name}.` : `Checked ${habit.name}.`,
    ));
    days.append(button);
  }
  item.append(head, numbers, days);
  return item;
}

function stat(value, label, live = false) {
  const span = document.createElement("span");
  if (live) span.className = "live";
  const strong = document.createElement("strong");
  strong.textContent = String(value);
  span.append(strong, label);
  return span;
}

async function act(change, message) {
  try {
    await change();
    await load();
    say(message);
  } catch (error) {
    say(error instanceof Error ? error.message : String(error), true);
  }
}

$("#add").addEventListener("submit", async (event) => {
  event.preventDefault();
  const input = $("#name");
  const name = input.value.trim();
  if (!name) return;
  await act(async () => {
    const result = await miniapp.actions.call("habits.create", { name });
    if (result.changes === 0) throw Error("You already track 20 habits. Archive one first.");
    input.value = "";
  }, `Added ${name}.`);
});

const session = await miniapp.auth.session();
if (session.role === "anonymous") {
  $("#gate").hidden = false;
  $("#signin").addEventListener("click", () => miniapp.auth.signIn());
} else {
  const who = $("#who");
  const email = document.createElement("span");
  email.textContent = session.name;
  const out = document.createElement("button");
  out.type = "button";
  out.textContent = "Sign out";
  out.addEventListener("click", async () => { await miniapp.auth.signOut(); location.reload(); });
  who.append(email, out);
  $("#app").hidden = false;
  await load().catch((error) => say(error.message, true));
}
