import { mkdir, readFile, writeFile } from "node:fs/promises";
import path from "node:path";
import { es, words } from "../examples/home/i18n.js";

const home = path.resolve(import.meta.dir, "../examples/home");

export function spanishHome(html: string) {
  let page = html.replace('<html lang="en">', '<html lang="es">');
  page = page.replace('<link rel="alternate" hreflang="en" href="./">', '<link rel="alternate" hreflang="en" href="../">');
  page = page.replace('<link rel="alternate" hreflang="es" href="es/">', '<link rel="alternate" hreflang="es" href="./">');
  page = page.replace('<a class="lang" href="es/" lang="es">Español</a>', '<a class="lang" href="../" lang="en">English</a>');
  page = page.replace(/<title>[\s\S]*?<\/title>/, `<title>${words.es.title}</title>`);
  page = page.replace(/<meta name="description" content="[^"]*">/, `<meta name="description" content="${words.es.description}">`);
  page = page.replace(/<meta property="og:title" content="[^"]*">/, `<meta property="og:title" content="${words.es.title}">`);
  page = page.replace(/<meta property="og:description" content="[^"]*">/, `<meta property="og:description" content="${words.es.description}">`);
  const replacements: { start: number; end: number; value: string }[] = [];
  const tags = /<([a-z][\w:-]*)\b[^>]*\bdata-t="([^"]+)"[^>]*>/gi;
  for (const match of page.matchAll(tags)) {
    const value = es[match[2]];
    if (!value) continue;
    const tag = match[1];
    const endTag = new RegExp(`<\\/?${tag}\\b[^>]*>`, "gi");
    endTag.lastIndex = match.index! + match[0].length;
    let depth = 1;
    let end = -1;
    for (let token = endTag.exec(page); token; token = endTag.exec(page)) {
      if (token[0][1] === "/") depth -= 1;
      else depth += 1;
      if (!depth) { end = token.index; break; }
    }
    if (end >= 0) replacements.push({ start: match.index! + match[0].length, end, value });
  }
  for (const replacement of replacements.sort((a, b) => b.start - a.start)) page = `${page.slice(0, replacement.start)}${replacement.value}${page.slice(replacement.end)}`;
  page = page.replace(/<[a-z][\w:-]*\b[^>]*\bdata-t-label="[^"]+"[^>]*>/gi, (tag) => {
    const key = /\bdata-t-label="([^"]+)"/i.exec(tag)?.[1];
    const label = /\baria-label="([^"]*)"/i.exec(tag)?.[1];
    if (!key || !label) return tag;
    return tag.replace(`aria-label="${label}"`, `aria-label="${words.es.labels[key] ?? label}"`);
  });
  page = page.replace(/<(a|link)\b[^>]*>/gi, (tag) => {
    if (/\bhreflang=/.test(tag)) return tag;
    return tag.replace(/\bhref="([^"]+)"/i, (attribute, href: string) => {
      if (/^(?:[a-z][a-z0-9+.-]*:|\/\/|#|\.\.\/)/i.test(href)) return attribute;
      return `href="../${href === "./" ? "" : href}"`;
    });
  });
  page = page.replace(/(<script\b[^>]*\bsrc=")([^"]+)(")/gi, (match, before: string, src: string, after: string) => `${before}../${src}${after}`);
  return page;
}

if (import.meta.main) {
  const source = await readFile(path.join(home, "index.html"), "utf8");
  const target = path.join(home, "pages/es/index.html");
  await mkdir(path.dirname(target), { recursive: true });
  await writeFile(target, spanishHome(source));
  console.log("Wrote examples/home/pages/es/index.html from examples/home/index.html and i18n.js.");
}
