// Every status line reads a failure the same way. A refusal's message is the
// sentence to show; the rate limit answers with a code and an unreachable
// Kernel with a browser error, so those two get a sentence here.
const reason = (error) => error?.message === 'rate-limited' ? 'Too many requests just now. Wait a few seconds and try again.'
  : error instanceof TypeError && /fetch|network|load failed/i.test(error.message) ? 'The app could not reach its server. Check the connection and try again.'
  : error?.message ?? String(error);
const form = document.querySelector('#join'), status = document.querySelector('#status'), count = document.querySelector('#count');
async function refresh() {
  try { const [row] = await miniapp.actions.call('waitlist.count'); count.textContent = row.n; } catch { count.textContent = '?'; }
}
form.addEventListener('submit', async (event) => {
  event.preventDefault();
  const button = form.querySelector('button'); button.disabled = true; status.className = 'status'; status.textContent = 'Saving…';
  const email = form.email.value.trim().toLowerCase(), name = form.name.value.trim();
  try {
    // The name uses a dash when the visitor leaves it empty.
    await miniapp.actions.call('waitlist.join', { email, name: name || '-', source: location.search.slice(1) || 'direct' });
    status.textContent = 'You are on the list. We will write when it is ready.'; form.reset(); await refresh();
  } catch (error) {
    status.className = 'status err';
    // waitlist.join declares a guard, so a repeated address is refused with its sentence.
    status.textContent = reason(error);
  } finally { button.disabled = false; }
});
refresh();
