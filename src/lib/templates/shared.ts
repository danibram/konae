import { appUiStyles, type AppUiVariant } from "./app-ui.generated";

export const templateVariant: AppUiVariant = "clean";

export function appStyles(extra = "", variant: AppUiVariant = templateVariant): string {
  return extra ? `${appUiStyles[variant]}\n${extra}\n` : appUiStyles[variant];
}

export const templateStyles = appStyles();

export const escapeHtmlSource = `function escapeHtml(value) {
  const entities = { '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;' };
  return String(value ?? '').replace(/[&<>"']/g, (char) => entities[char]);
}`;

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
