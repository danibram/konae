import { limitIndex, unboundedQueryReason } from "./query-bound.ts";
import { sqlTokens } from "./sql-tokens.ts";

// Points the links at this installation. The page answers on <domain>/,
// on <domain>/p/<name>/ and on its own <slug>-<6 hex>.<domain>, and only the
// browser knows which, so the addresses come from location. A custom domain
// looks like the bare domain here, so on one the links name that domain.
const { protocol, hostname, port, pathname } = location;
const labels = hostname.split(".");
const ownHostname = !pathname.startsWith("/p/") && labels.length > 1 && /-[0-9a-f]{6}$/.test(labels[0]);
const domain = ownHostname ? labels.slice(1).join(".") : hostname;
const at = (host, path) => `${protocol}//${host}${port ? `:${port}` : ""}${path}`;
const docs = (page) => at(domain, `/p/docs/${page}`);
const reduced = matchMedia("(prefers-reduced-motion: reduce)").matches;

const links = {
  editor: at(`app.${domain}`, "/"),
  mcp: at(`app.${domain}`, "/mcp"),
  "editor-origin": at(`app.${domain}`, ""),
  docs: docs(""),
  llms: docs("llms.txt"),
  "first-app": docs("guides/first-app-from-the-editor"),
  move: docs("guides/take-the-app-elsewhere"),
  checks: docs("gate-and-kernel"),
  cli: docs("guides/edit-files-with-the-cli"),
  operate: docs("operate"),
  "mcp-guide": docs("mcp-guide"),
};
for (const element of document.querySelectorAll("[data-link]")) element.href = links[element.dataset.link];
for (const element of document.querySelectorAll("[data-text]")) element.textContent = links[element.dataset.text];

// Every code block copies its own text. Without clipboard access it selects the text instead.
for (const button of document.querySelectorAll("[data-copy]")) {
  const pre = button.closest("figure").querySelector("pre");
  // Each command line wraps under itself on a narrow screen, so a wrapped line
  // never reads as a new command. The copied text keeps the original lines.
  const original = pre.textContent;
  const lines = pre.innerHTML.split("\n");
  pre.innerHTML = lines.map((line) => `<span class="line">${line || " "}</span>`).join("");
  pre.dataset.text = original;
  button.addEventListener("click", async () => {
    try {
      await navigator.clipboard.writeText(pre.dataset.text);
      button.textContent = "Copied";
    } catch {
      const range = document.createRange();
      range.selectNodeContents(pre);
      getSelection().removeAllRanges();
      getSelection().addRange(range);
      button.textContent = /Mac|iPhone|iPad/.test(navigator.platform) ? "Press ⌘C" : "Press Ctrl+C";
    }
    setTimeout(() => { button.textContent = "Copy"; }, 2400);
  });
}

// In the ways band, pointing at "14 rows" lights "LIMIT 14" too, and back.
const marks = [...document.querySelectorAll(".doors mark")];
for (const mark of marks) {
  mark.addEventListener("mouseenter", () => marks.forEach((other) => other.classList.add("linked")));
  mark.addEventListener("mouseleave", () => marks.forEach((other) => other.classList.remove("linked")));
}

// The statement box runs the row-limit check the editor's Validation Gate
// runs (query-bound.ts is a copy of src/lib/query-bound.ts), with the sentence
// src/kernel/actions.ts and the Kernel build from it. The Gate checks a
// statement's shape first; this box says so instead of judging other statements.
const check = document.querySelector("[data-check]");
if (check) {
  const toggle = check.querySelector("[data-toggle]");
  const input = check.querySelector("textarea");
  const verdict = check.querySelector("[data-verdict]");
  const who = check.querySelector("[data-who]");
  const answer = check.querySelector("[data-answer]");

  const foot = check.querySelector(".statement-foot span");
  const accepted = (sql) => {
    const top = sqlTokens(sql).filter((token) => token.depth === 0);
    const at = limitIndex(top);
    if (at !== -1) {
      const count = top[at + 1]?.text;
      if (!/^\d+$/.test(count ?? "")) return "It names a LIMIT. Both accept it.";
      return `It reads at most ${count} ${count === "1" ? "row" : "rows"}. The number is yours to choose. Both accept it.`;
    }
    if (!top.some((token) => token.kind === "word" && token.text.toUpperCase() === "FROM")) return "The statement reads no table, so the row limit does not apply.";
    return "A total with no GROUP BY returns one row, so both accept it.";
  };
  const judge = () => {
    const sql = input.value.trim();
    const select = /^(select|with)\b/i.test(sql);
    const reason = select ? unboundedQueryReason(sql) : null;
    check.classList.toggle("dropped", Boolean(reason));
    check.classList.toggle("idle", !select);
    verdict.textContent = !select ? "Not checked here" : reason ? "Refused" : "Accepted";
    who.textContent = reason ? "By the editor and by the server, word for word" : !select ? "Row-limit check" : "By the editor and by the server";
    answer.textContent = !sql
      ? "Write a SELECT to check it."
      : !select
      ? "Write a SELECT. The editor checks writes with other rules."
      : reason
        ? `Action players.list ${reason} Add a LIMIT. A total (count, sum, avg, min or max) with no GROUP BY does not need one.`
        : accepted(sql);

    const limited = select && !reason && limitIndex(sqlTokens(sql).filter((token) => token.depth === 0)) !== -1;
    toggle.style.visibility = "visible";
    toggle.textContent = reason ? "Add the LIMIT" : "Remove the LIMIT";
    foot.textContent = "Edit the statement, or";
    foot.parentElement.hidden = !(reason || limited);
  };
  check.querySelector("[data-static]").hidden = true;
  input.hidden = false;
  toggle.hidden = false;
  input.addEventListener("input", judge);
  // The LIMIT goes before a trailing comment, where the Gate would read it, and comes out the same way.
  toggle.addEventListener("click", () => {
    const [code, ...comment] = input.value.trimEnd().split(/(?=--)/);
    const statement = code.trimEnd();
    const bare = statement.replace(/\s+LIMIT\s+\S+$/i, "");
    input.value = [bare === statement ? `${statement} LIMIT 14` : bare, ...comment].join(" ");
    judge();
  });
  judge();
}

// The example app moves once when the other server comes into view: someone
// signs up on this site, then a package carries the app to the other server,
// where it answers at a new address with the same rows. Afterwards the visitor
// can add players where the app lives and move it on purpose. The slot holding
// the app is as tall as the app and the empty one folds to one line; both
// change together while a small copy of the card flies between them, so the
// stage keeps its height.
const stage = document.querySelector(".stage");
if (stage) {
  const app = stage.querySelector("[data-app]");
  const slots = [...stage.querySelectorAll("[data-slot] .slot")];
  const url = stage.querySelector("[data-url]");
  const meta = stage.querySelector("[data-meta]");
  const meter = stage.querySelector("[data-meter]");
  const row = stage.querySelector("[data-dana]");
  const who = row.querySelector("b");
  const note = row.querySelector("[data-dana-note]");
  const more = stage.querySelector("[data-more]");
  const newest = stage.querySelector("[data-newest]");
  const visited = new Set([0]);
  const chip = stage.querySelector("[data-package]");
  const form = stage.querySelector("[data-join]");
  const field = form.querySelector("input");
  const add = form.querySelector("[data-add]");
  const formNote = form.querySelector("[data-join-note]");
  const mover = stage.querySelector("[data-move]");
  const announce = stage.querySelector("[data-announce]");
  const hosts = ["konae.example.com", "apps.example.net"];
  const addresses = ["football-3f9a2c.konae.example.com", "football-71c0e4.apps.example.net"];
  const places = 14;
  let names = [];
  let at = 0;
  let busy = false;
  let moving = false;
  const wait = (ms) => new Promise((resolve) => setTimeout(resolve, ms));
  const total = () => 9 + names.length;

  const reserve = () => {
    const pad = parseFloat(getComputedStyle(slots[0]).paddingTop) + parseFloat(getComputedStyle(slots[0]).paddingBottom);
    stage.style.setProperty("--slot-full", `${app.offsetHeight + pad}px`);
  };
  // Dana keeps her row; the newest player after her gets the next one, and
  // anyone in between is named in the "more" line.
  const short = (name) => (name.length > 14 ? `${name.slice(0, 14).replace(/\s+\S*$/, "")}…` : name);
  const render = (state, newestState = "signed up") => {
    const [first, ...rest] = names;
    const latest = rest[rest.length - 1];
    const between = rest[rest.length - 2];
    who.textContent = short(first ?? "Dana");
    row.classList.toggle("in", Boolean(first));
    note.textContent = first ? (latest ? newestNote(state) : state) : "signing up…";
    newest.classList.toggle("in", Boolean(latest));
    newest.querySelector("b").textContent = latest ? short(latest) : "";
    newest.querySelector("span").textContent = latest ? newestState : "";
    const shown = 2 + (first ? 1 : 0) + (latest ? 1 : 0);
    more.textContent = `and ${total() - shown} more${between ? `, including ${short(between)}` : ""}`;
    meta.textContent = `${total()} of ${places} places taken`;
    const full = total() >= places;
    field.disabled = full;
    if (full) formNote.textContent = "The list is full.";
    meter.style.width = `${(total() / places) * 100}%`;
  };
  const newestNote = (state) => (state === "still here" ? "still here" : "signed up");
  const place = (index) => {
    at = index;
    slots[index].prepend(app);
    url.textContent = addresses[index];
    for (const [i, slot] of slots.entries()) slot.classList.toggle("empty", i !== index);
    visited.add(index);
    for (const [i, slot] of slots.entries()) slot.querySelector(".slot-empty").textContent = visited.has(i) ? "Nothing here now" : "Nothing here yet";
    stage.classList.toggle("moved", index === 1);
  };
  const controls = () => {
    mover.textContent = moving ? "Moving…" : `Move to ${hosts[1 - at]}`;
    if (busy) mover.setAttribute("aria-disabled", "true"); else mover.removeAttribute("aria-disabled");
    add.textContent = total() >= places ? "Start over" : "Add";
  };
  // The slots swap height with a transition while a half-size copy of the card
  // flies straight down (or up) the connector, behind the package chip.
  const carry = async (index) => {
    if (reduced) { place(index); return; }
    const from = app.getBoundingClientRect();
    const base = stage.getBoundingClientRect();
    // Measure where the app lands without animating, then start from the old layout.
    stage.classList.add("instant");
    place(index);
    const to = app.getBoundingClientRect();
    place(1 - index);
    void stage.offsetHeight;
    stage.classList.remove("instant");
    const ghost = app.cloneNode(true);
    ghost.removeAttribute("data-app");
    ghost.classList.add("ghost");
    ghost.setAttribute("aria-hidden", "true");
    Object.assign(ghost.style, { left: `${from.left - base.left}px`, top: `${from.top - base.top}px`, width: `${from.width}px` });
    stage.append(ghost);
    app.style.visibility = "hidden";
    // Out of the flow while it flies, so each slot follows only its own animated height.
    app.style.position = "absolute";
    app.style.width = `${from.width}px`;
    place(index);
    chip.classList.add(index === 1 ? "down" : "up");
    await wait(20);
    ghost.style.scale = "0.5";
    await wait(180);
    ghost.style.translate = `0px ${to.top - from.top}px`;
    await wait(420);
    ghost.style.scale = "1";
    await wait(260);
    app.style.position = "";
    app.style.width = "";
    app.style.visibility = "";
    ghost.remove();
    chip.classList.remove("down", "up");
    url.classList.add("fresh");
    setTimeout(() => url.classList.remove("fresh"), 1200);
  };
  const move = async () => {
    if (busy) return;
    busy = true;
    moving = true;
    controls();
    const next = 1 - at;
    chip.textContent = "Package: accounts, data, files, rules";
    stage.classList.remove("arrived");
    await carry(next);
    chip.textContent = "Same accounts, data, files, rules.";
    stage.classList.add("arrived");
    if (names.length) render("still here", "still here");
    announce.textContent = `The app moved to ${hosts[next]} with the same accounts, data, files and rules.${names.length ? ` ${names.join(", ")} ${names.length === 1 ? "is" : "are"} still on the list.` : ""}`;
    busy = false;
    moving = false;
    controls();
  };
  const join = async (name) => {
    if (names.length) {
      newest.classList.add("in");
      newest.querySelector("b").textContent = short(name);
      newest.querySelector("span").textContent = "signing up…";
    }
    if (!reduced) await wait(500);
    names.push(name);
    render("signed up");
    announce.textContent = `${name} signed up on ${hosts[at]}.`;
    controls();
  };

  reserve();
  addEventListener("resize", reserve);
  place(0);
  // Without motion nothing plays, so Dana is already on the list.
  if (reduced) names.push("Dana");
  render(reduced ? "signed up" : "signing up…");
  form.hidden = false;
  mover.hidden = false;
  controls();
  mover.addEventListener("click", move);
  form.addEventListener("submit", async (event) => {
    event.preventDefault();
    if (busy) return;
    formNote.textContent = "";
    if (total() >= places) {
      names = ["Dana"];
      visited.clear();
      if (at === 1) { stage.classList.add("instant"); place(0); void stage.offsetHeight; stage.classList.remove("instant"); } else place(0);
      chip.textContent = "Package: accounts, data, files, rules";
      stage.classList.remove("arrived");
      field.value = "";
      render("signed up");
      formNote.textContent = "";
      announce.textContent = "The list starts over on konae.example.com with Dana.";
      controls();
      field.focus();
      return;
    }
    const name = field.value.trim().slice(0, 16);
    if (!name) { formNote.textContent = "Type a name first."; field.focus(); return; }
    field.value = "";
    busy = true;
    await join(name);
    busy = false;
    controls();
  });
  if (!reduced && "IntersectionObserver" in window) {
    const seen = new IntersectionObserver(async (entries) => {
      if (!entries.some((entry) => entry.isIntersecting)) return;
      seen.disconnect();
      await wait(1500);
      if (at !== 0 || busy || names.length) return;
      busy = true;
      await join("Dana");
      await wait(1200);
      busy = false;
      await move();
    }, { threshold: 0.6 });
    seen.observe(slots[1].closest(".server"));
  }
}

// The header's button hides while the hero's own "Open the editor" is on screen.
const heroCta = document.querySelector(".hero-actions");
const barCta = document.querySelector(".bar-cta");
if (heroCta && barCta && "IntersectionObserver" in window) {
  new IntersectionObserver((entries) => barCta.classList.toggle("quiet", entries.some((entry) => entry.isIntersecting))).observe(heroCta);
}
