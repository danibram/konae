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
    status.textContent = /UNIQUE/.test(error.message) ? 'That address is already on the list.' : error.message;
  } finally { button.disabled = false; }
});
refresh();
