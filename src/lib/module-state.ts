export type ModuleRefusal = { code: string; message: string };

type ModuleCase = {
  rule: string;
  module: string;
  state?: unknown;
  expected?: unknown;
  switch?: boolean;
  known?: boolean;
  required?: string;
  requiredState?: { on?: boolean };
  used?: boolean;
  option?: string;
  value?: unknown;
  options?: Array<{ key: string; type: string; values?: string[] }>;
};

function stable(value: unknown): unknown {
  if (Array.isArray(value)) return value.map(stable);
  if (value !== null && typeof value === "object") {
    return Object.fromEntries(Object.entries(value).sort(([left], [right]) => left.localeCompare(right)).map(([key, item]) => [key, stable(item)]));
  }
  return value;
}

export function checkModuleChange(input: ModuleCase): ModuleRefusal | null {
  if (input.rule === "expected" && JSON.stringify(stable(input.state)) !== JSON.stringify(stable(input.expected))) {
    return { code: "module.changed", message: `Module ${input.module} changed since it was read. Read modules_read and try again.` };
  }
  if (input.rule === "switch" && input.switch !== true) {
    return { code: "module.not-switch", message: `Module ${input.module} cannot be changed yet.` };
  }
  if (input.rule === "known" && input.known !== true) return { code: "module.unknown", message: `Unknown module ${input.module}.` };
  if (input.rule === "on" && (input.state as { on?: boolean } | undefined)?.on !== true) {
    return { code: "module.off", message: `Module ${input.module} is off. Turn it on with module_set.` };
  }
  if (input.rule === "requires" && input.requiredState?.on !== true) {
    return { code: "module.requires", message: `Module ${input.module} requires ${input.required}, which is off. Turn ${input.required} on first.` };
  }
  if (input.rule === "in-use" && input.used === true) {
    return { code: "module.in-use", message: `Module ${input.module} is in use by the published app. Remove its use before turning it off.` };
  }
  if (input.rule === "option-unknown" || input.rule === "option-invalid") {
    const option = input.options?.find((candidate) => candidate.key === input.option);
    if (!option) return { code: "module.option-unknown", message: `Unknown option ${input.option} for module ${input.module}.` };
    const valid = option.type === "boolean" ? typeof input.value === "boolean"
      : option.type === "string" ? typeof input.value === "string"
      : option.type === "number" ? typeof input.value === "number"
      : option.type === "enum" ? option.values?.includes(String(input.value)) === true
      : option.type === "path" ? input.value === null || (typeof input.value === "string" && input.value.length > 0 && !input.value.startsWith("/") && !input.value.includes("\\") && input.value.split("/").every((part) => part !== "" && part !== "." && part !== ".."))
      : false;
    if (!valid) {
      const message = option.type === "enum"
        ? `Module ${input.module} option ${input.option} must be one of ${option.values?.join(", ") ?? ""}.`
        : option.type === "path"
          ? `Module ${input.module} option ${input.option} must be a relative app path or null.`
          : `Module ${input.module} option ${input.option} must be a ${option.type}.`;
      return { code: "module.option-invalid", message };
    }
  }
  return null;
}
