/** The stylesheet and page helpers the newer templates share, in Tiny Tasks' visual language. */
export const templateStyles = `
@import url('https://fonts.googleapis.com/css2?family=DM+Serif+Display:ital@0;1&family=Manrope:wght@500;600;700&display=swap');

:root { color-scheme: light dark; font-family: Manrope, sans-serif; background: #f3efe5; color: #24251f; }
* { box-sizing: border-box; }
body { margin: 0; min-height: 100vh; background: radial-gradient(circle at 90% 8%, #d7e6b7 0 18%, transparent 38%), #f3efe5; }
.app-shell { width: min(680px, calc(100% - 40px)); margin: 0 auto; padding: 28px 0 48px; }
header { display: flex; justify-content: space-between; align-items: flex-start; gap: 24px; }
.eyebrow, .section-label { font-size: 11px; letter-spacing: .18em; font-weight: 700; text-transform: uppercase; }
h1 { font-family: 'DM Serif Display', serif; font-size: clamp(32px, 7vw, 56px); line-height: .9; letter-spacing: -.045em; margin: 8px 0 20px; font-weight: 400; }
.intro { max-width: 420px; color: #77756c; line-height: 1.6; margin: 0 0 24px; }
.identity { display: flex; align-items: center; gap: 8px; font-size: 11px; font-weight: 700; }
button, input, textarea { font: inherit; }
.primary { border: 1px solid #24251f; padding: 10px 16px; background: #c8f34a; color: #171910; font-weight: 700; cursor: pointer; }
.quiet { border: 1px solid #24251f; border-radius: 999px; padding: 7px 12px; background: transparent; color: inherit; font-size: 11px; font-weight: 700; cursor: pointer; }
.card { background: #fffdf8; border: 1px solid #24251f; box-shadow: 6px 6px 0 #24251f; padding: 16px; display: grid; gap: 12px; margin-bottom: 40px; }
.card label { display: grid; gap: 6px; font-size: 12px; font-weight: 700; }
.card input, .card textarea { border: 1px solid #c9c5b9; background: transparent; color: inherit; padding: 10px 12px; outline: none; }
.card textarea { min-height: 110px; resize: vertical; }
.section-label { display: flex; justify-content: space-between; border-bottom: 1px solid #969487; padding: 18px 0 10px; }
.rows { display: grid; }
.row { display: grid; grid-template-columns: 1fr auto; gap: 12px; align-items: center; padding: 16px 4px; border-bottom: 1px solid #c9c5b9; }
.row p { margin: 0; overflow-wrap: anywhere; line-height: 1.5; }
.row small { display: block; margin-top: 4px; color: #77756c; }
.row button { border: 0; background: transparent; color: #9d3929; font-size: 12px; text-decoration: underline; cursor: pointer; }
.empty { color: #77756c; padding: 20px 4px; line-height: 1.5; }
.status { min-height: 20px; color: #9d3929; font-size: 13px; }
[hidden] { display: none !important; }
@media (prefers-color-scheme: dark) {
  :root { background: #1c1b17; color: #f3efe5; }
  body { background: #1c1b17; }
  .card { background: #2a2822; border-color: #f3efe5; box-shadow: 4px 4px 0 #0a0c10; }
  .card input, .card textarea, .row { border-color: #5c594f; }
  .quiet { border-color: #f3efe5; }
  .section-label { border-color: #5c594f; }
  .intro, .row small, .empty { color: #d9d4c8; }
  .status, .row button { color: #ff9b85; }
}
`;

/** Page helpers every template's app.js starts with. */
export const templateHelpers = `const $ = (selector) => document.querySelector(selector);
const statusLine = $('#status');

function escapeHtml(value) {
  const node = document.createElement('span');
  node.textContent = value ?? '';
  return node.innerHTML;
}

function when(ms) {
  return new Date(ms).toLocaleString();
}

function showError(error) {
  statusLine.textContent = error instanceof Error ? error.message : String(error);
}
`;
