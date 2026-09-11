document.querySelector('#login').onclick = () => miniapp.auth.signIn();
miniapp.auth.session().then(user => document.querySelector('#result').textContent = user.name);
