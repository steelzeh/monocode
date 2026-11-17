# Tweaks

A tweak is a small add-on of your own. It keeps personal ideas out of the app itself. A tweak can add:

- a native panel in the right side panel
- slash commands
- items in a chat's right-click menu

Open **Settings → Tweaks**. Press **Add from GitHub** and paste `owner/repo` or a link to the tweak's folder, or press **Open folder** and put each tweak in its own folder, then press **Reload**. Every tweak is off until you turn it on, and the page shows what it adds and what it may do.

## Files

```
tweaks/
  standup/
    tweak.json
    panel.js
```

`tweak.json`:

```json
{
  "id": "standup",
  "name": "Standup",
  "version": "1.0.0",
  "author": "your-name",
  "description": "Write my standup.",
  "permissions": ["composer.insert", "send_prompts"],
  "panel": { "title": "Standup", "script": "panel.js", "icon": "sparkles" },
  "commands": [
    { "name": "standup", "description": "Write today's standup", "prompt": "Write my standup about {{args}}" }
  ],
  "menu": [
    { "id": "summary", "label": "Summarize this chat", "prompt": "Summarize this chat in five lines." }
  ]
}
```

A panel's optional `icon` names one of the app's icons, from `crates/monocode-gpui/assets/hugeicons`, without `.svg`. A name the app does not have shows the default icon.

The `id` must match the folder name and may use `a-z`, `0-9` and hyphens. Unknown fields are refused, so a typo shows up as an error in Settings.

Commands and menu items with a `prompt` need the `send_prompts` permission. A slash command sends its `prompt` to the agent. `{{args}}` becomes what you typed after the command; without it, that text follows the prompt. The chat shows what you typed. Names the app or your agent already uses keep their usual meaning.

A menu item sends its `prompt` to the chat you right-clicked. Leave out `prompt` to handle the click in the tweak's own script instead; `busy_label` is the row's text while the script works on that chat.

## Reaching the app

A tweak reaches the app in one way: `tweak.app(action, input, callback)`. The action is one of the app's general actions, and it must be listed in the tweak's `permissions`. The callback gets `(result, error)`.

```js
tweak.app("worktrees.list", {}, (result, error) => { /* ... */ });
```

| Action or permission | What it does | Input |
| --- | --- | --- |
| `sessions.list` | Lists this project's chats | |
| `sessions.read` | Reads a chat you clicked | `sessionId`, `limit` (1 to 3 turns), `before`, `maxChars` |
| `sessions.rename` | Renames a chat you clicked | `sessionId`, `title` |
| `sessions.setWorking` | Makes a chat's title shimmer until the tweak is done | `sessionId`, `working` |
| `worktrees.list` | Lists this project's worktrees, and `focused`, the selected one | |
| `worktrees.select` | Switches worktree; leave out `path` to show them all | `path` |
| `composer.insert` | Adds text to your message box | `text` |
| `models.ask` | Asks your background model; the answer is in `result.text` | `prompt` |
| `send_prompts` | Lets the tweak's commands and menu items send their prompts; not callable | |

## Panels

`panel.js` is the body of a function that returns a render function, as in [session visuals](visuals.md). It has the same `visual` components, plus `tweak`:

```js
let chats = [];
tweak.onRefresh(() => tweak.app("sessions.list", {}, (result) => { chats = result.sessions; }));
return () => visual.column([
  visual.text(`${tweak.context.project}: ${chats.length} chats`),
  visual.button("Add a note", () => tweak.app("composer.insert", { text: "A note" })),
]);
```

`tweak.onRefresh` runs when the panel opens and when you move to another chat, project or worktree. The panel draws again after each answer, and keeps its own variables.

## Scripts

A tweak is self-contained: its whole feature lives in its folder. Add `"script": "main.js"` to run code when one of your menu items is clicked. A script has no view and no `visual` components.

```js
tweak.onMenu("shorten", (chat) => {
  tweak.app("models.ask", { prompt: `Shorten this title: ${chat.title}` }, (answer, error) => {
    if (error) throw new Error(error);
    tweak.app("sessions.rename", { sessionId: chat.id, title: answer.text });
  });
});
```

`chat` has `id`, `title` and `project`. Throw an `Error` to show its message in the app.

## Security

A tweak can reach only what this page lists, and you decide before it runs.

- **Sandbox.** Scripts have no files, network, shell or modules. They run only from one of their handlers.
- **Permissions.** Each ability has a permission, shown before you turn the tweak on. The app enforces them, whatever the script asks for. A tweak that breaks a rule is stopped and says why.
- **Clicks.** Actions that change something need a click. Only reads can run when a panel loads.
- **Your chats.** Actions on a chat work only on a chat you clicked the tweak on.
- **Spending.** One click can make 16 app calls, and 4 of them can use tokens. A tweak can have 8 calls running. Turning a tweak off cancels its running model calls.
- **Prompts are powerful.** A prompt goes to your agent with your agent's access to files and commands. That is why prompts need `send_prompts`, and why **Show prompts** and the install screen show their full text. Read them.
- **Changes.** Turning a tweak on records a fingerprint of its files. If they change, including through an update, the tweak turns off and says so until you turn it on again.
- **Installing.** The app downloads one exact commit and only the files the manifest names, shows you what they do, and writes nothing until you press **Install**. Tweaks never update on their own; add one again to update it.

## Limits

A panel script runs in the same sandbox as session visuals, with 250 ms per call.

Tweaks cannot change existing parts of the app. See [`examples/tweaks`](../../examples/tweaks) for a working example.
