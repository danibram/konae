const page = document.querySelector("#posts");
if (page) {
  const rows = await miniapp.actions.call("posts.published");
  for (const row of rows) {
    const item = document.createElement("li");
    const link = document.createElement("a");
    link.href = miniapp.page.url(`/posts/${encodeURIComponent(row.slug)}`);
    link.textContent = row.slug;
    item.append(link);
    page.append(item);
  }
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
