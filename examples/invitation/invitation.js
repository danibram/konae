// Points the links at this installation. The page answers on
// <domain>/p/<name>/ and on its own hostname, so the domain comes from location.
const { protocol, hostname, port, pathname } = location;
const labels = hostname.split(".");
const ownHostname = !pathname.startsWith("/p/") && labels.length > 1 && /-[0-9a-f]{6}$/.test(labels[0]);
const domain = ownHostname ? labels.slice(1).join(".") : hostname;
const at = (path) => `${protocol}//${domain}${port ? `:${port}` : ""}${path}`;
const links = {
  home: at("/"),
  docs: at("/p/docs/"),
  "first-app": at("/p/docs/guides/first-app-from-the-editor"),
  privacy: at("/p/politica-de-privacidad/"),
  terms: at("/p/terminos-y-condiciones/"),
};
const point = () => { for (const element of document.querySelectorAll("[data-link]")) element.href = links[element.dataset.link]; };

// The page is written in English and carries its Spanish text here. ?lang=
// wins, then the visitor's last choice, then the browser's language.
const es = {
  skip: "Saltar al contenido",
  docs: "Docs",
  title: "Pide una invitación",
  lede: "Mientras el sitio es pequeño, hacer apps aquí es por invitación. Deja tu correo y quien lleva el sitio te envía un código. Lo usas una vez, para crear tu cuenta.",
  p1: "Tu correo se usa para enviarte el código y para nada más.",
  p2: "También necesitarás una clave de IA de Anthropic, OpenAI u OpenRouter. El asistente funciona con tu propia cuenta en ellos.",
  p3: "¿Prefieres no esperar? Konae es código abierto, y <a href=\"https://github.com/danibram/konae\">puedes montarlo en tu servidor</a> con Docker.",
  email: "Correo",
  building: "¿Qué harías con él? <i>(opcional)</i>",
  send: "Pedir un código",
  "done.title": "Tu petición está apuntada",
  "done.text": "El código te llega por correo cuando quien lleva este sitio lo envíe. Mientras tanto, la guía de la primera app enseña qué harás con él.",
  "done.guide": "Leer la guía de la primera app",
  "done.home": "Volver a la portada",
  "how.title": "Este formulario es una app de Konae",
  "how.text": "Tiene una tabla y una Server Action. Cualquier visitante puede ejecutar la acción, la app rechaza un correo que ya pidió código, y cada fila nueva envía un correo al propietario de la app.",
  privacy: "Política de privacidad",
  terms: "Términos",
};
const words = {
  en: { other: "Español", page: "Request an invitation · Konae", saving: "Sending…", repeated: "That address has already asked, or it is not an email address.", busy: "Too many requests just now. Wait a few seconds and try again.", offline: "The app could not reach its server. Check the connection and try again." },
  es: { other: "English", page: "Pide una invitación · Konae", saving: "Enviando…", repeated: "Ese correo ya pidió un código, o no es una dirección de correo.", busy: "Demasiadas peticiones ahora mismo. Espera unos segundos y prueba otra vez.", offline: "La app no pudo llegar a su servidor. Revisa la conexión y prueba otra vez." },
};
const marked = [...document.querySelectorAll("[data-t]")];
const english = new Map(marked.map((element) => [element, element.innerHTML]));
const stored = (() => { try { return localStorage.getItem("konae-home-lang"); } catch { return null; } })();
let lang = [new URLSearchParams(location.search).get("lang"), stored, navigator.language?.slice(0, 2)].find((value) => value === "en" || value === "es") ?? "en";
const form = document.querySelector("[data-form]");
const status = document.querySelector("[data-status]");
const done = document.querySelector("[data-done]");
const switcher = document.querySelector("[data-lang]");
// The last thing the status line said, by name, so a change of language says it again.
let said = "";
const speak = () => {
  const w = words[lang];
  document.documentElement.lang = lang;
  document.title = w.page;
  for (const element of marked) element.innerHTML = lang === "es" && es[element.dataset.t] ? es[element.dataset.t] : english.get(element);
  switcher.textContent = w.other;
  status.textContent = w[said] ?? said;
  point();
};
switcher.hidden = false;
switcher.addEventListener("click", () => {
  lang = lang === "es" ? "en" : "es";
  try { localStorage.setItem("konae-home-lang", lang); } catch {}
  speak();
});

// A refusal's message is the guard's sentence. The rate limit answers with a
// code and an unreachable Kernel with a browser error, so those get their own.
const guard = "That address has already asked, or it is not an email address.";
form.addEventListener("submit", async (event) => {
  event.preventDefault();
  const button = form.querySelector("button");
  button.disabled = true;
  status.classList.remove("bad");
  said = "saving";
  speak();
  try {
    await miniapp.actions.call("requests.create", { email: form.email.value.trim(), building: form.building.value.trim() || "-", lang });
    form.hidden = true;
    done.hidden = false;
    done.querySelector("h2").focus?.();
  } catch (error) {
    status.classList.add("bad");
    said = error?.message === guard ? "repeated" : error?.message === "rate-limited" ? "busy" : error instanceof TypeError ? "offline" : error?.message ?? String(error);
    speak();
  } finally {
    button.disabled = false;
  }
});

// The privacy and terms links show only where the installation has those pages.
for (const element of document.querySelectorAll("[data-legal]")) {
  fetch(links[element.dataset.link], { method: "HEAD" }).then((response) => { element.hidden = !response.ok; }).catch(() => {});
}

speak();
