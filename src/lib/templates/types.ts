import type { DeploymentVisibility } from "../../deployments/manifest";

export type TemplateFile = "index.html" | "styles.css" | "app.js" | "schema.sql" | "actions.json";

export type TemplateSource = {
  id: string;
  name: string;
  sentence: string;
  visibility: DeploymentVisibility;
  files: Record<TemplateFile, string>;
};
