// The inbox reads and acknowledges events through declared Server Actions only.
// Events arrive on the signed route POST /hooks/events, which this page never calls.

const $ = (selector) => document.querySelector(selector);
const list = $("#events");
const empty = $("#empty");
const notice = $("#notice");
const filters = $("#filters");
const time = new Intl.DateTimeFormat(undefined, { dateStyle: "medium", timeStyle: "short" });

function show(message) {
  notice.textContent = message;
  notice.hidden = !message;
}

function currentFilter() {
  const form = new FormData(filters);
  // An empty choice is sent as null, which the nullable inputs bind as SQL NULL.
  return {
    severity: form.get("severity") || null,
    state: form.get("state") || null,
  };
}

function element(tag, className, text) {
  const node = document.createElement(tag);
  if (className) node.className = className;
  if (text !== undefined) node.textContent = text;
  return node;
}

function renderEvent(event) {
  const item = element("li", event.acknowledged_at ? "event done" : "event");
  item.dataset.severity = event.severity;

  const main = element("div", "event-main");
  const head = element("div", "event-head");
  head.append(element("span", `badge ${event.severity}`, event.severity), element("h2", "event-title", event.title));
  main.append(head);
  if (event.body) main.append(element("p", "event-body", event.body));
  main.append(element("p", "event-meta", `${event.source} · ${event.external_id} · ${time.format(event.received_at)}`));

  const side = element("div", "event-side");
  if (event.acknowledged_at) {
    const who = event.acknowledged_by_email ?? "a teammate";
    side.append(element("p", "acked", `Acknowledged by ${who}\n${time.format(event.acknowledged_at)}`));
  } else {
    const button = element("button", "button primary", "Acknowledge");
    button.type = "button";
    button.addEventListener("click", () => acknowledge(event, button));
    side.append(button);
  }
  item.append(main, side);
  return item;
}

async function load() {
  show("");
  try {
    const filter = currentFilter();
    const [rows, [counts]] = await Promise.all([
      miniapp.actions.call("events.list", filter),
      miniapp.actions.call("events.counts"),
    ]);
    $("#total-count").textContent = counts.total;
    $("#ack-count").textContent = counts.acknowledged;
    $("#open-count").textContent = counts.total - counts.acknowledged;
    list.replaceChildren(...rows.map(renderEvent));
    empty.hidden = rows.length > 0;
    $("#empty-text").textContent = counts.total === 0
      ? "No event has arrived yet. Services post them to /hooks/events."
      : "No event matches these filters.";
  } catch (error) {
    show(`Could not load events: ${error.message}`);
  }
}

async function acknowledge(event, button) {
  button.disabled = true;
  try {
    // The action records event.acknowledged with the actor when it changes the row,
    // and its guard refuses when someone acknowledged the event first.
    await miniapp.actions.call("events.acknowledge", { id: event.id });
  } catch (error) {
    show(error.message);
  }
  await load();
}

const session = await miniapp.auth.session();
if (session.role === "anonymous") {
  await miniapp.auth.signIn();
} else {
  $("#who").textContent = `Signed in as ${session.name}`;
  // The Kernel binds only the actor's ID, so the page states its own email for the
  // "acknowledged by" line. The value is what this client sends, not a verified claim.
  try { await miniapp.actions.call("members.hello", { email: session.name }); } catch {}
  filters.addEventListener("change", load);
  $("#refresh").addEventListener("click", load);
  $("#sign-out").addEventListener("click", async () => {
    await miniapp.auth.signOut();
    location.replace("/");
  });
  await load();
  setInterval(() => { if (document.visibilityState === "visible") load(); }, 20_000);
}
