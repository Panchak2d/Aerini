# Keyboard Shortcuts

Every shortcut below appears in the app itself: click **Shortcuts** in the toolbar (next to Credentials and Settings), or press **?** anywhere on the canvas, to open the same list. Two rows are exceptions to that. One notes a second working key combo the in-app list leaves out — `Backspace` for Delete, already documented in [Getting Started](../getting-started/getting-started.md). The other, `Ctrl+Shift+C` for Chat, is missing from the in-app list altogether, not just a second combo for an action already there — confirmed directly against `toolbar.ts`. Everything else here matches the in-app list exactly.

## Canvas

| Action | Shortcut |
|---|---|
| Search / command palette | `Space` or `Ctrl+K` |
| Pan canvas | Middle-drag |
| Zoom | Scroll |
| Fit to screen | `Ctrl+Shift+F` |
| Focus mode — hides the sidebar for a distraction-free canvas | `F` (not while editing a field) |
| Close panel, from an empty area of the canvas | Double-click |

## Selection & Editing

| Action | Shortcut |
|---|---|
| Box select | Shift-drag |
| Select all | `Ctrl+A` |
| Duplicate | `Ctrl+D` |
| Delete | `Del` or `Backspace` |
| Undo — the toolbar's **Undo** button does the same, and its tooltip names the step. Covers adding, deleting, moving, and wiring nodes, plus renaming, disabling, choosing a saved credential, and changing a node's settings. History is per open workflow and is cleared when you switch to another | `Ctrl+Z` |
| Redo — also a toolbar button | `Ctrl+Y` or `Ctrl+Shift+Z` |
| Cut connector | Alt-drag across it |
| Move where a wire ends — drag from the wired input, or click the wire and drag the ring on its input end; dropping anywhere else cancels, and `Esc` cancels mid-drag. On an input that takes many wires, such as Merge's, click the wire first | Drag |
| Wire backwards — drag from an empty input to an output, or release over empty canvas to pick a node to connect into it | Drag |
| Move every wire leaving an output to another output — drag from the output, then drop on a different output; one `Ctrl+Z` undoes it | `Ctrl`/`Cmd`-drag |
| Disconnect a wire — click it and press `Del`, or right-click it | `Del` |

## Workflow

| Action | Shortcut |
|---|---|
| Save | `Ctrl+S` |
| New workflow | `Ctrl+N` |
| Run workflow | `Ctrl+Enter` |
| Open [Chat](chat-panel.md) — needs a Webhook and an Output node | `Ctrl+Shift+C` |
| Show this shortcut list | `?` |
| Navigate the workflow list | `↑` `↓` |
| Open the selected item in the list | `Enter` |
| Rename an item in the list | Double-click |
| Open a workflow row's menu | Right-click |

## Node Config

| Action | Shortcut |
|---|---|
| Open a node's config panel | Double-click the node |
| Close the config panel | `Esc` |
| Insert node data into any field | `{{` |

On macOS, `Ctrl` in the table above is `Cmd`; both work everywhere Aerini checks for one.

## See also

- [Getting Started](../getting-started/getting-started.md) uses several of these (`Ctrl+S`, `Delete`) as they come up naturally while you build your first workflow.
- [Expressions](expressions.md) covers what `{{` inserts and how to build on it.
- [Nodes Reference](nodes.md) for what actually lives inside a node's config panel once you open it.
- [Chat Panel](chat-panel.md) for what `Ctrl+Shift+C` opens and why it's conditional on the workflow's nodes.
