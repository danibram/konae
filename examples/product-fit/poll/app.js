// Every status line reads a failure the same way. A refusal's message is the
// sentence to show; the rate limit answers with a code and an unreachable
// Kernel with a browser error, so those two get a sentence here.
const reason = (error) => error?.message === 'rate-limited' ? 'Too many requests just now. Wait a few seconds and try again.'
  : error instanceof TypeError && /fetch|network|load failed/i.test(error.message) ? 'The app could not reach its server. Check the connection and try again.'
  : error?.message ?? String(error);
const options = ['Tuesday', 'Thursday', 'Saturday', 'Sunday'];
const $ = (s) => document.querySelector(s);
const voted = localStorage.getItem('voted');
$('#options').innerHTML = options.map(o => `<button data-o="${o}" ${voted ? 'disabled' : ''}>${o}${voted === o ? ' ✓' : ''}</button>`).join('');
$('#options').addEventListener('click', async (e) => {
  const b = e.target.closest('button[data-o]'); if (!b) return;
  for (const button of $('#options').querySelectorAll('button')) button.disabled = true;
  try { await miniapp.actions.call('votes.cast', { choice: b.dataset.o }); localStorage.setItem('voted', b.dataset.o); location.reload(); }
  catch (err) { $('#status').textContent = reason(err); for (const button of $('#options').querySelectorAll('button')) button.disabled = false; }
});
async function report() {
  const [rows, hours] = await Promise.all([miniapp.actions.call('votes.results'), miniapp.actions.call('votes.perhour')]);
  const total = rows.reduce((n, r) => n + r.n, 0);
  $('#total').textContent = `${total} votes`;
  $('#bars').innerHTML = options.map(o => { const n = rows.find(r => r.choice === o)?.n ?? 0; return `<div class="bar"><span>${o}</span><i style="width:${(100 * n / Math.max(1, total)).toFixed(1)}%"></i><span>${n}</span></div>`; }).join('');
  const max = Math.max(1, ...hours.map(h => h.n));
  // votes.perhour groups by the UTC hour; the label shows it in the reader's own time.
  const local = (hour) => new Date(`${hour.replace(' ', 'T')}:00:00Z`).toLocaleString(undefined, { weekday: 'short', hour: '2-digit', minute: '2-digit' });
  $('#hours').innerHTML = hours.map(h => `<b style="height:${(100 * h.n / max).toFixed(0)}%" title="${local(h.hour)}: ${h.n}"></b>`).join('');
}
const refresh = () => report().catch((err) => { $('#status').textContent = reason(err); });
refresh(); setInterval(refresh, 5000);
