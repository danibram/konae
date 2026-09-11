import { expect, test } from "bun:test";
import { createHostedArtifact } from "./artifact";

test("a hosted artifact contains compiled code and the named-action SDK without private definitions", () => {
  const artifact = createHostedArtifact({
    body: "<main id=app></main>",
    styles: "main { color: red }",
    script: "document.querySelector('#app').textContent = 'compiled nested module';",
    deploymentId: "deployment_1",
  });

  expect(artifact.files["index.html"]).toContain("compiled nested module");
  expect(artifact.files["index.html"]).toContain("/_littlebox/actions/");
  expect(artifact.files["index.html"]).not.toContain("SELECT ");
  expect(artifact.entrypoint).toBe("index.html");
});
