// Everything this feature does is in this file. It reaches the app only through
// the general actions listed in tweak.json.
const BUDGET = 8000;
const MESSAGE_LIMIT = 2000;

const PROMPT = `Generate a title that will help the user recognize this coding session weeks later.
Do not call tools. Reply with the title only.

Before answering, silently reduce the conversation to:
- Subject: What system, feature, or problem is this really about?
- Outcome: What does the user ultimately want to understand or change?
- Incidental instructions: What only describes how the agent should do the work?

Title the subject and outcome. Discard incidental instructions.

Editorial rules:
- 3-8 words, fewer than 40 characters.
- Use a compact noun phrase or clear action phrase.
- Capture the umbrella goal when the request lists several symptoms or steps.
- Name the product change, not the mock, plan, report, branch, or PR used to produce it.
- Models, subagents, tools, and output formats do not belong in the title unless they are themselves the topic.
- Do not claim the work is complete.
- Do not copy and truncate the user's message.
- Avoid quotes, labels, filler, and trailing punctuation.

Title the whole conversation below, not only its first request. Prefer the subject the user is working on now.

Conversation:
`;

function section(turn) {
  const parts = [`USER:\n${turn.user.text}`];
  if (turn.assistant) parts.push(`ASSISTANT:\n${turn.assistant.text}`);
  return parts.join("\n\n");
}

// The first turns and the newest ones, each once, until the budget runs out.
function conversation(first, newest) {
  const seen = new Set();
  const parts = [];
  let remaining = BUDGET;
  for (const turn of [...first, ...newest]) {
    if (seen.has(turn.turnId)) continue;
    seen.add(turn.turnId);
    const text = section(turn);
    if (text.length > remaining) break;
    remaining -= text.length;
    parts.push(text);
  }
  return parts.join("\n\n");
}

function clean(answer) {
  const line = answer.trim().split("\n")[0].trim().replace(/^['"`]+|['"`]+$/g, "").replace(/\s+/g, " ");
  return line.length <= 50 ? line : `${line.slice(0, 47).trimEnd()}...`;
}

const stopOn = (error) => {
  if (error) throw new Error(error);
};

tweak.onMenu("regenerate", (chat) => {
  const read = { sessionId: chat.id, limit: 3, maxChars: MESSAGE_LIMIT };
  tweak.app("sessions.setWorking", { sessionId: chat.id, working: true });
  tweak.app("sessions.read", read, (newest, error) => {
    stopOn(error);
    tweak.app("sessions.read", { ...read, before: 3 }, (first, error) => {
      stopOn(error);
      const text = conversation(first.turns, newest.turns);
      if (!text) throw new Error("This chat has no messages to title yet.");
      tweak.app("models.ask", { prompt: PROMPT + text }, (answer, error) => {
        stopOn(error);
        const title = clean(answer.text);
        if (!title) throw new Error("The model returned no title.");
        tweak.app("sessions.rename", { sessionId: chat.id, title });
      });
    });
  });
});
