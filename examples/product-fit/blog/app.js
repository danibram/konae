const page = document.querySelector("#posts");
if (page) {
  const rows = await miniapp.actions.call("posts.published");
  page.innerHTML = rows.map((row) => `<li><a href="/posts/${encodeURIComponent(row.slug)}">${row.slug}</a></li>`).join("");
} else {
  const { slug } = miniapp.page.params;
  const [post] = await miniapp.actions.call("posts.head", { slug });
  if (post) {
    document.title = post.title;
    document.querySelector("#title").textContent = post.title;
    document.querySelector("#summary").textContent = post.description;
    document.querySelector("#body").textContent = post.body;
  }
}
