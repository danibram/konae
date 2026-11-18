import { marked } from "marked";

export function renderMarkdown(markdown) {
  return marked.parse(markdown, { gfm: true });
}

export function descriptionOf(markdown) {
  const html = renderMarkdown(markdown);
  const paragraph = /<p>([\s\S]*?)<\/p>/.exec(html)?.[1] ?? "";
  const text = paragraph
    .replace(/<[^>]+>/g, "")
    .replace(/&amp;/g, "&")
    .replace(/&lt;/g, "<")
    .replace(/&gt;/g, ">")
    .replace(/&quot;/g, '"')
    .replace(/&#39;|&apos;/g, "'")
    .replace(/\s+/g, " ")
    .trim();
  if (text.length <= 160) return text;
  const cut = text.slice(0, 160).lastIndexOf(" ");
  return text.slice(0, cut > 0 ? cut : 160);
}
