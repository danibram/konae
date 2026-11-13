import { limitIndex, unboundedQueryReason } from "./query-bound.ts";
import { sqlTokens } from "./sql-tokens.ts";
import { es, words } from "./i18n.js";

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
  pieces: docs("pieces"),
  examples: docs("examples"),
};
const point = () => {
  for (const element of document.querySelectorAll("[data-link]")) element.href = links[element.dataset.link];
};
point();
for (const element of document.querySelectorAll("[data-text]")) element.textContent = links[element.dataset.text];

// The page is written in English and carries its Spanish text in i18n.js.
// ?lang= wins, then the visitor's last choice, then the browser's language.
// Each part of the page that writes text registers a function in `redraw`,
// which runs again when the language changes.
const marked = [...document.querySelectorAll("[data-t]")];
const english = new Map(marked.map((element) => [element, element.innerHTML]));
const stored = (() => { try { return localStorage.getItem("konae-home-lang"); } catch { return null; } })();
const asked = new URLSearchParams(location.search).get("lang");
let lang = [asked, stored, navigator.language?.slice(0, 2)].find((value) => value === "en" || value === "es") ?? "en";
let w = words[lang];
const redraw = [];
const speak = () => {
  w = words[lang];
  document.documentElement.lang = lang;
  document.title = w.title;
  document.querySelector('meta[name="description"]').content = w.description;
  for (const element of marked) element.innerHTML = lang === "es" && es[element.dataset.t] ? es[element.dataset.t] : english.get(element);
  for (const element of document.querySelectorAll("[data-t-label]")) element.setAttribute("aria-label", w[element.dataset.tLabel.split(".")[1]]);
  point();
  for (const draw of redraw) draw();
};
const switcher = document.querySelector("[data-lang]");
redraw.push(() => { switcher.textContent = w.other; switcher.lang = lang === "es" ? "en" : "es"; });
switcher.hidden = false;
switcher.addEventListener("click", () => {
  lang = lang === "es" ? "en" : "es";
  try { localStorage.setItem("konae-home-lang", lang); } catch {}
  const here = new URL(location.href);
  here.searchParams.set("lang", lang);
  history.replaceState(null, "", here);
  speak();
});

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
      button.dataset.state = "copied";
    } catch {
      const range = document.createRange();
      range.selectNodeContents(pre);
      getSelection().removeAllRanges();
      getSelection().addRange(range);
      button.dataset.state = "press";
    }
    label();
    setTimeout(() => { button.dataset.state = ""; label(); }, 2400);
  });
  const label = () => {
    const state = button.dataset.state;
    button.textContent = state === "copied" ? w.copied : state === "press" ? w.press(/Mac|iPhone|iPad/.test(navigator.platform) ? "⌘C" : "Ctrl+C") : w.copy;
  };
  redraw.push(label);
}

// In the ways band, pointing at "14 rows" lights "LIMIT 14" too, and back.
// A change of language writes the first mark again, so the marks are found
// each time.
const doors = document.querySelector(".doors");
const light = (on) => (event) => {
  if (!event.target.closest?.("mark")) return;
  for (const mark of doors.querySelectorAll("mark")) mark.classList.toggle("linked", on);
};
doors.addEventListener("mouseover", light(true));
doors.addEventListener("mouseout", light(false));

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
      if (!/^\d+$/.test(count ?? "")) return w.namesLimit;
      return w.readsAtMost(count);
    }
    if (!top.some((token) => token.kind === "word" && token.text.toUpperCase() === "FROM")) return w.noTable;
    return w.oneRow;
  };
  const judge = () => {
    const sql = input.value.trim();
    const select = /^(select|with)\b/i.test(sql);
    const reason = select ? unboundedQueryReason(sql) : null;
    check.classList.toggle("dropped", Boolean(reason));
    check.classList.toggle("idle", !select);
    verdict.textContent = !select ? w.notChecked : reason ? w.refused : w.accepted;
    who.textContent = reason ? w.byBothExact : !select ? w.rowCheck : w.byBoth;
    input.placeholder = w.placeholder;
    answer.textContent = !sql
      ? w.writeSelect
      : !select
      ? w.onlySelect
      : reason
        ? `Action players.list ${reason} Add a LIMIT. A total (count, sum, avg, min or max) with no GROUP BY does not need one.`
        : accepted(sql);

    const limited = select && !reason && limitIndex(sqlTokens(sql).filter((token) => token.depth === 0)) !== -1;
    toggle.style.visibility = "visible";
    toggle.textContent = reason ? w.addLimit : w.removeLimit;
    foot.textContent = w.editOr;
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
  redraw.push(judge);
}

// The example app plays once when the stage comes into view: the description
// types itself, the app appears at its address, someone signs up, then a
// package carries the app to the other server, where it answers at a new
// address with the same rows. Afterwards the visitor can add players where the
// app lives and move it on purpose. The slot holding the app is as tall as the
// app and the empty one folds to one line; both change together while a small
// copy of the card flies between them, so the stage keeps its height.
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
  const typed = stage.querySelector("[data-typed]");
  const status = stage.querySelector("[data-status]");
  const prompts = { en: typed.textContent, es: es["stage.prompt"] };
  const hosts = ["konae.example.com", "apps.example.net"];
  const addresses = ["football.konae.example.com", "football.apps.example.net"];
  const places = 14;
  let names = [];
  let at = 0;
  let busy = false;
  let moving = false;
  // What the stage last showed, kept as names of states so a change of
  // language can write the same moment again.
  let letters = Infinity;
  let phase = "";
  let rowState = "joining";
  let newestState = "joined";
  let pending = "";
  let arrived = false;
  let formSays = "";
  const wait = (ms) => new Promise((resolve) => setTimeout(resolve, ms));
  const total = () => 9 + names.length;

  const reserve = () => {
    const pad = parseFloat(getComputedStyle(slots[0]).paddingTop) + parseFloat(getComputedStyle(slots[0]).paddingBottom);
    stage.style.setProperty("--slot-full", `${app.offsetHeight + pad}px`);
  };
  // Dana keeps her row; the newest player after her gets the next one, and
  // anyone in between is named in the "more" line.
  const short = (name) => (name.length > 14 ? `${name.slice(0, 14).replace(/\s+\S*$/, "")}…` : name);
  const render = () => {
    const [first, ...rest] = names;
    const latest = rest[rest.length - 1];
    const between = rest[rest.length - 2];
    typed.textContent = prompts[lang].slice(0, letters);
    status.textContent = phase ? w[phase] : "";
    who.textContent = short(first ?? "Dana");
    row.classList.toggle("in", Boolean(first));
    note.textContent = first ? w[latest && rowState !== "still" ? "joined" : rowState] : w.joining;
    newest.classList.toggle("in", Boolean(latest || pending));
    newest.querySelector("b").textContent = pending ? short(pending) : latest ? short(latest) : "";
    newest.querySelector("span").textContent = pending ? w.joining : latest ? w[newestState] : "";
    const shown = 2 + (first ? 1 : 0) + (latest ? 1 : 0);
    more.textContent = w.more(total() - shown, between ? short(between) : "");
    meta.textContent = w.places(total(), places);
    const full = total() >= places;
    field.disabled = full;
    field.placeholder = w.yourName;
    formNote.textContent = full ? w.full : formSays ? w[formSays] : "";
    meter.style.width = `${(total() / places) * 100}%`;
    chip.textContent = arrived ? w.arrived : w.package;
    for (const [i, slot] of slots.entries()) slot.querySelector(".slot-empty").textContent = visited.has(i) ? w.emptyNow : w.emptyYet;
    mover.textContent = moving ? w.moving : w.moveTo(hosts[1 - at]);
    if (busy) mover.setAttribute("aria-disabled", "true"); else mover.removeAttribute("aria-disabled");
    add.textContent = full ? w.startOver : w.add;
  };
  const place = (index) => {
    at = index;
    slots[index].prepend(app);
    url.textContent = addresses[index];
    for (const [i, slot] of slots.entries()) slot.classList.toggle("empty", i !== index);
    visited.add(index);
    stage.classList.toggle("moved", index === 1);
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
    arrived = false;
    stage.classList.remove("arrived");
    render();
    const next = 1 - at;
    await carry(next);
    arrived = true;
    stage.classList.add("arrived");
    if (names.length) { rowState = "still"; newestState = "still"; }
    announce.textContent = w.saidMoved(hosts[next], names);
    busy = false;
    moving = false;
    render();
  };
  const join = async (name) => {
    if (names.length) { pending = name; render(); }
    if (!reduced) await wait(500);
    pending = "";
    names.push(name);
    rowState = "joined";
    newestState = "joined";
    render();
    announce.textContent = w.saidJoined(name, hosts[at]);
  };
  // The description types itself, then the app appears where it was hidden.
  const describe = async () => {
    for (letters = 0; letters < prompts[lang].length; letters += 1) { render(); await wait(26); }
    letters = Infinity;
    phase = "writing";
    render();
    await wait(900);
    stage.classList.remove("building");
    phase = "published";
    render();
    url.classList.add("fresh");
    setTimeout(() => url.classList.remove("fresh"), 1200);
  };

  reserve();
  addEventListener("resize", reserve);
  place(0);
  // Without motion nothing plays, so Dana is already on the list.
  if (reduced) { names.push("Dana"); rowState = "joined"; phase = "published"; }
  form.hidden = false;
  mover.hidden = false;
  redraw.push(() => { render(); reserve(); });
  mover.addEventListener("click", move);
  form.addEventListener("submit", async (event) => {
    event.preventDefault();
    if (busy) return;
    formSays = "";
    if (total() >= places) {
      names = ["Dana"];
      visited.clear();
      if (at === 1) { stage.classList.add("instant"); place(0); void stage.offsetHeight; stage.classList.remove("instant"); } else place(0);
      arrived = false;
      rowState = "joined";
      stage.classList.remove("arrived");
      field.value = "";
      render();
      announce.textContent = w.saidReset;
      field.focus();
      return;
    }
    const name = field.value.trim().slice(0, 16);
    if (!name) { formSays = "typeName"; render(); field.focus(); return; }
    field.value = "";
    busy = true;
    await join(name);
    busy = false;
    render();
  });
  if (!reduced && "IntersectionObserver" in window) {
    letters = 0;
    busy = true;
    stage.classList.add("building");
    const seen = new IntersectionObserver(async (entries) => {
      if (!entries.some((entry) => entry.isIntersecting)) return;
      seen.disconnect();
      await wait(700);
      await describe();
      await wait(1300);
      await join("Dana");
      await wait(1400);
      busy = false;
      await move();
    }, { threshold: 0.3 });
    seen.observe(stage);
  }
}

// Bands and cards rise into place the first time they come into view, each a
// little after the one before it in the same row. Without the observer, or
// with reduced motion, nothing is hidden.
const rising = [...document.querySelectorAll("[data-reveal]")];
if (!reduced && "IntersectionObserver" in window) {
  document.documentElement.classList.add("anim");
  const rise = new IntersectionObserver((entries) => {
    for (const entry of entries) {
      if (!entry.isIntersecting) continue;
      entry.target.classList.add("seen");
      rise.unobserve(entry.target);
    }
  }, { rootMargin: "0px 0px -8% 0px" });
  for (const element of rising) {
    const row = [...element.parentElement.children].filter((child) => child.hasAttribute("data-reveal"));
    element.style.setProperty("--i", String(row.indexOf(element) % 4));
    rise.observe(element);
  }
}

// The header's button hides while the hero's own "Open the editor" is on screen.
const heroCta = document.querySelector(".hero-actions");
const barCta = document.querySelector(".bar-cta");
if (heroCta && barCta && "IntersectionObserver" in window) {
  new IntersectionObserver((entries) => barCta.classList.toggle("quiet", entries.some((entry) => entry.isIntersecting))).observe(heroCta);
}

speak();
