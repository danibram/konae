const policy = {
  public: {
    title: 'Public deployment',
    note: '<strong>The page can open.</strong><br>The database stays behind the rules and answers only declared actions.',
    shell: '<span class="yes" id="shell-access">allowed</span>'
  },
  private: {
    title: 'Private deployment',
    note: '<strong>A signed-in person can open it.</strong><br>Without a session the page, the actions, and the files stay closed.',
    shell: '<span class="no" id="shell-access">needs a grant</span>'
  }
};

document.querySelectorAll('[data-mode]').forEach((button) => {
  button.addEventListener('click', () => {
    document.querySelectorAll('[data-mode]').forEach((item) => item.classList.toggle('active', item === button));
    const state = policy[button.dataset.mode];
    document.querySelector('#policy-title').textContent = state.title;
    document.querySelector('#policy-note').innerHTML = state.note;
    document.querySelector('#shell-access').outerHTML = state.shell;
  });
});

const observer = new IntersectionObserver((entries) => {
  entries.forEach((entry) => {
    if (entry.isIntersecting) {
      entry.target.classList.add('visible');
      observer.unobserve(entry.target);
    }
  });
}, { threshold: .12 });
document.querySelectorAll('.reveal').forEach((element) => observer.observe(element));
