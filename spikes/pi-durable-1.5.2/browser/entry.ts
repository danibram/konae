const output = document.querySelector<HTMLPreElement>("#result")!;
document.querySelector("#run")!.addEventListener("click", async () => {
  output.textContent = "loading";
  try {
    const { runCompatibilityFixture } = await import("./runtime");
    output.textContent = JSON.stringify(await runCompatibilityFixture(), null, 2);
  } catch (error) {
    output.textContent = JSON.stringify({ error: String(error), stack: error instanceof Error ? error.stack : "" });
  }
});
