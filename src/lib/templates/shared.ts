import { appUiStyles, type AppUiVariant } from "./app-ui.generated";

/** The variant a template is born in. Change it here, or swap the stylesheet for appUiStyles.brutal in the Capsule. */
export const templateVariant: AppUiVariant = "clean";

/** A Capsule's styles.css: the design system in one variant, then the app's own rules. */
export function appStyles(extra = "", variant: AppUiVariant = templateVariant): string {
  return extra ? `${appUiStyles[variant]}\n${extra}\n` : appUiStyles[variant];
}

/** The stylesheet the newer templates share. */
export const templateStyles = appStyles();

/** The escapeHtml every template's app.js carries. It escapes quotes too, so a value is safe in an attribute. */
export const escapeHtmlSource = `function escapeHtml(value) {
  const entities = { '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;' };
  return String(value ?? '').replace(/[&<>"']/g, (char) => entities[char]);
}`;

/** Page helpers every template's app.js starts with. */
export const templateHelpers = `const $ = (selector) => document.querySelector(selector);
const statusLine = $('#status');

${escapeHtmlSource}

function when(ms) {
  return new Date(ms).toLocaleString();
}

function say(message, tone) {
  statusLine.textContent = message;
  if (tone) statusLine.dataset.tone = tone;
  else delete statusLine.dataset.tone;
}

function showError(error) {
  say(error instanceof Error ? error.message : String(error), 'danger');
}
`;
