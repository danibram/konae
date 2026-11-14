import { parseServerActionSource, type ServerActionDefinition } from "../kernel/actions";

type SuggestionFacts = {
  files: Record<string, string | undefined>;
  manifest: { visibility: "public" | "private"; fileAccess?: "app-users" };
};

/**
 * Changes Describe offers before anyone types, each with the test that says the
 * app already does it. actions.json and the manifest are enough to tell, so the
 * list never offers what the app has.
 */
const candidates: { sentence: string; done: (actions: ServerActionDefinition[], facts: SuggestionFacts) => boolean }[] = [
  {
    sentence: "Let anyone with the link add a row.",
    done: (actions, facts) => facts.manifest.visibility === "private" || actions.some((action) => action.kind === "mutation" && action.access === "anonymous"),
  },
  {
    sentence: "Ask people to sign in before the app opens.",
    done: (_actions, facts) => facts.manifest.visibility === "private",
  },
  {
    sentence: "Email me when someone adds a row.",
    done: (actions) => actions.some((action) => action.notify),
  },
  {
    sentence: "Let people mark a row as done.",
    done: (actions) => actions.some((action) => /(^|\.)(toggle|done|complete)/.test(action.name)),
  },
  {
    sentence: "Let people attach a photo to each row.",
    done: (_actions, facts) => facts.manifest.fileAccess === "app-users" || /miniapp\.files/.test(facts.files["app.js"] ?? ""),
  },
  {
    sentence: "Clear rows older than 30 days every night.",
    done: (actions) => actions.some((action) => action.schedule),
  },
  {
    sentence: "Add a search box above the list.",
    done: (actions) => actions.some((action) => /search/.test(action.name)),
  },
];

export function suggestedChanges(facts: SuggestionFacts, count = 3): string[] {
  let actions: ServerActionDefinition[] = [];
  try {
    actions = Object.values(parseServerActionSource(facts.files["actions.json"] ?? "").actions);
  } catch {
    // A file the check refuses still gets suggestions; none of them is marked done.
  }
  if (actions.some((action) => action.name.startsWith("messages."))) {
    return [
      ...(!actions.some((action) => action.notify) ? ["Email me when someone sends a message."] : []),
      ...(!actions.some((action) => /^messages\.(read|mark.*read)$/i.test(action.name)) ? ["Let me mark messages as read."] : []),
      ...(!actions.some((action) => /search/i.test(action.name)) ? ["Add a search box above the messages."] : []),
      ...(!actions.some((action) => /archive/i.test(action.name)) ? ["Let me archive messages I have handled."] : []),
    ].slice(0, count);
  }
  return candidates.filter((candidate) => !candidate.done(actions, facts)).slice(0, count).map((candidate) => candidate.sentence);
}
