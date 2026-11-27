export type FileIntakeConsumer = { id: string; source: string; action: string; input?: Record<string, unknown> };
export type FileIntakeDeclaration = { version: number; sources: Array<{ id: string; version: number; replay?: string }>; consumers: FileIntakeConsumer[] };

const idPattern = /^[A-Za-z0-9_-]{1,64}$/;

export function validateFileIntake(value: unknown): FileIntakeDeclaration {
  if (!value || typeof value !== "object" || Array.isArray(value)) throw new Error("file-intake.json must be an object.");
  const declaration = value as Partial<FileIntakeDeclaration>;
  if (declaration.version !== 1) throw new Error("file-intake.json version must be 1.");
  if (!Array.isArray(declaration.sources) || declaration.sources.length < 1 || declaration.sources.length > 32) throw new Error("file-intake.json must declare between 1 and 32 sources.");
  if (!Array.isArray(declaration.consumers) || declaration.consumers.length > 32) throw new Error("file-intake.json may declare at most 32 consumers.");
  const sources = new Set<string>();
  for (const source of declaration.sources) {
    if (!source || !idPattern.test(source.id)) throw new Error("File intake source ID must use 1 to 64 ASCII letters, digits, '_' or '-'.");
    if (source.version !== 1) throw new Error(`File intake source ${source.id} version must be 1.`);
    if (source.replay && source.replay !== "delivery-key-or-current-path-sha256") throw new Error(`File intake source ${source.id} has an unsupported replay rule.`);
    if (sources.has(source.id)) throw new Error(`file-intake.json declares source ${source.id} twice.`);
    sources.add(source.id);
  }
  const consumers = new Set<string>();
  for (const consumer of declaration.consumers) {
    if (!consumer || !idPattern.test(consumer.id)) throw new Error("File intake consumer ID must use 1 to 64 ASCII letters, digits, '_' or '-'.");
    if (!idPattern.test(consumer.source)) throw new Error("File intake source ID must use 1 to 64 ASCII letters, digits, '_' or '-'.");
    if (!sources.has(consumer.source)) throw new Error(`File intake consumer ${consumer.id} names undeclared source ${consumer.source}.`);
    if (!/^[A-Za-z0-9_-]{1,64}\.[A-Za-z0-9_-]{1,64}$/.test(consumer.action)) throw new Error(`File intake consumer ${consumer.id} must name a declared mutation action.`);
    if (consumers.has(consumer.id)) throw new Error(`file-intake.json declares consumer ${consumer.id} twice.`);
    if (consumer.input !== undefined && (!consumer.input || typeof consumer.input !== "object" || Array.isArray(consumer.input))) throw new Error("File intake consumer input must be an object.");
    consumers.add(consumer.id);
  }
  return declaration as FileIntakeDeclaration;
}

export function validateFileIntakeConsumers(declaration: FileIntakeDeclaration, actions: Record<string, { kind?: string; bindings?: Array<{ source: string; key?: string; nullable?: boolean }> }>): void {
  for (const consumer of declaration.consumers) {
    const action = actions[consumer.action];
    if (!action) throw new Error(`File intake consumer ${consumer.id} names missing action ${consumer.action}.`);
    if (action.kind !== "mutation") throw new Error(`File intake consumer ${consumer.id} action ${consumer.action} must be a mutation.`);
    if ((action.bindings ?? []).some((binding) => binding.source === "actor")) throw new Error(`File intake consumer ${consumer.id} action cannot bind a human actor.`);
    const allowed = new Set((action.bindings ?? []).filter((binding) => binding.source === "input").map((binding) => binding.key));
    if (Object.keys(consumer.input ?? {}).some((key) => ["versionId", "sourceId", "path", "sha256", "size"].includes(key))) throw new Error(`File intake consumer ${consumer.id} input uses a reserved version field.`);
    if (Object.keys(consumer.input ?? {}).some((key) => !allowed.has(key))) throw new Error(`File intake consumer ${consumer.id} input has an undeclared action field.`);
    for (const binding of (action.bindings ?? []).filter((item) => item.source === "input")) {
      if (binding.key && !binding.nullable && !(binding.key in (consumer.input ?? {})) && !["versionId", "sourceId", "path", "sha256", "size"].includes(binding.key)) {
        throw new Error(`File intake consumer ${consumer.id} does not supply action input ${binding.key}.`);
      }
    }
  }
}
