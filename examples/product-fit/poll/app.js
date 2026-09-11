const options = ['Tuesday', 'Thursday', 'Saturday', 'Sunday'];
const $ = (s) => document.querySelector(s);
const voted = localStorage.getItem('voted');
$('#options').innerHTML = options.map(o => `<button data-o="${o}" ${voted ? 'disabled' : ''}>${o}${voted === o ? ' ✓' : ''}</button>`).join('');
$('#options').addEventListener('click', async (e) => {
  const b = e.target.closest('button[data-o]'); if (!b) return;
  try { await miniapp.actions.call('votes.cast', { choice: b.dataset.o }); localStorage.setItem('voted', b.dataset.o); location.reload(); }
  catch (err) { $('#status').textContent = err.message; }
});
async function report() {
  const [rows, hours] = await Promise.all([miniapp.actions.call('votes.results'), miniapp.actions.call('votes.perhour')]);
  const total = rows.reduce((n, r) => n + r.n, 0);
  $('#total').textContent = `${total} votes`;
  $('#bars').innerHTML = options.map(o => { const n = rows.find(r => r.choice === o)?.n ?? 0; return `<div class="bar"><span>${o}</span><i style="width:${(100 * n / Math.max(1, total)).toFixed(1)}%"></i><span>${n}</span></div>`; }).join('');
  const max = Math.max(1, ...hours.map(h => h.n));
  $('#hours').innerHTML = hours.map(h => `<b style="height:${(100 * h.n / max).toFixed(0)}%" title="${h.hour}: ${h.n}"></b>`).join('');
}
report(); setInterval(report, 5000);
