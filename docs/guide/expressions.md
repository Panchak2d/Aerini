# Expressions

An **[expression](../glossary.md#expression)** is the `{{...}}` syntax you type into a node's settings to pull in a value from somewhere else, instead of typing a fixed one. Type `{{HTTP Request.output.body}}` into a Send Email node's Body field, and that field sends whatever the HTTP Request node's response body actually was on that run, not a string you typed ahead of time.

This page covers what you can put inside `{{...}}`: node outputs, a handful of special variables, and a small set of built-in functions. For which config fields accept expressions at all, see the note at the top of [Nodes Reference](nodes.md).

## How it works

Aerini resolves every `{{...}}` in a node's config right before that node runs, using whatever data exists at that point in the workflow: earlier nodes' outputs, workflow variables set so far, and the current run's own metadata.

Only text fields carry expressions. A number field or a dropdown just has nowhere to type `{{`, so those always take a fixed value.

**An expression that can't be resolved never crashes the workflow.** It resolves to an empty string, and a warning explaining why lands in that run's log, things like "no node named 'X' in this workflow" or "field path not found." Check the run's log first if a field came out blank when you expected a value. In the desktop app, a reference to a missing node, a node that isn't connected before this one, or a field that node doesn't have is also flagged under **Problems** in the status bar before you run, with a one-click fix for a likely typo, and a field that came out empty in the last run is flagged there too.

You can mix expressions with plain text in the same field: `Order {{HTTP Request.output.order_id}} shipped` resolves to `Order 4521 shipped`. A field can also contain several expressions at once.

## Referencing another node's output

The general shape is:

```
{{Node Name.output}}
{{Node Name.output.field}}
{{Node Name.output.field.nested}}
{{Node Name.output.array_field[0]}}
```

**Node Name** is the name you gave the node on the canvas (its display label), not any internal ID. Renaming a node in the desktop app rewrites the `{{Old Name.…}}` references in your other nodes for you, as one undo step. References written outside the app (a hand-edited file, the API or MCP) aren't rewritten.

- `{{Node Name.output}}` on its own returns the node's entire output, stringified. For a simple value that's just the value; for an object or array, you get its JSON text.
- Add a dot-path after `.output` to reach one field: `{{Fetch Data.output.body.name}}`.
- Add `[N]` to index into an array at any point in the path: `{{List Items.output.items[0]}}`, or `{{List Items.output.items[0].tags[1]}}` for a nested one.

In the desktop app you don't have to type a reference by hand. Press **Insert data** under a text field, or type `@` at the start of a word, and pick a field from the nodes connected before this one; typing after the `@` searches node names, field names and the values from the last run. An `@` straight after another character, as in an email address, is left alone. If you type or paste text that is exactly one value from the last run, a small prompt offers to turn it into a live reference (**Convert**) or leave it as text (**Dismiss**). It never converts without your click, and Convert is one undo step.

The picker also lists nodes that are not connected to this one yet, under **Not connected**. Choose one and Aerini draws the wire from its output into this node and inserts its main result, as one undo step. Only wires that are safe are offered: never the node itself, never a node that comes after this one (that would loop), and nothing when this node's input already has a wire that a new one would replace.

A few things worth knowing about node references specifically:

- **If two nodes share the same name**, the expression resolves against whichever one comes first and logs a warning about the ambiguity. The desktop app numbers a repeated name (`Notify Me 2`) when you add, duplicate or rename a node, but a workflow file written elsewhere can still contain duplicates, so give nodes distinct names.
- **A node name can't contain `.`, `{`, `}`, `,`, `'` or `"`, can't start with `$`, and can't have a `(` straight after a plain word (`Code(JS)`; `Code (JS)` is fine).** The expression is split on `.` and read as a function call or a quoted value in those cases, so the node could never be referenced. The desktop app refuses these names when you rename a node.
- **A node that hasn't produced output yet** (it's later in the chain, or on a branch that hasn't run) resolves to empty string with a warning, it's not an error you need to catch.
- **A two-segment expression only works if the second segment is literally `output`.** `{{Fetch Data.output}}` is valid; `{{Fetch Data.result}}` is not, since that's read as a too-short path missing the `.output.` middle segment, and resolves to empty with a warning rather than guessing what you meant.

## Special variables

Three prefixes, all starting with `$`, cover things that aren't a node's output:

| Expression | Resolves to |
|---|---|
| `{{$run.id}}` | The current run's execution ID. |
| `{{$run.timestamp}}` | The current time, as an ISO 8601 timestamp, at the moment the expression resolves. |
| `{{$run.workflow_name}}` | This workflow's name. |
| `{{$vars.key}}` | A workflow variable set earlier in this same run by a [Set Variable](nodes.md#set-variable) node. Missing key resolves to empty string with a warning, same as any other unresolvable expression. |
| `{{$env.VAR_NAME}}` | An environment variable on the machine running the workflow. |

`$env` needs an explicit allowlist to work at all, and that allowlist only exists in one place: running `aerini-server api` with `--allow-env-vars VAR_ONE,VAR_TWO` on the command line. In the desktop app, and in `aerini-server serve` mode, there's no allowlist to pass, so every `{{$env...}}` expression resolves to empty with a warning telling you to pass `--allow-env-vars`, regardless of whether the variable actually exists on the machine.

## Functions

Wrap a value in a function call to transform it: `{{upper(Fetch Data.output.name)}}`. Arguments are comma-separated; a quoted argument (single or double quotes) is a literal string, an unquoted number is a literal number, and anything else unquoted is resolved as its own nested expression first. Function calls can nest: `{{upper(trim(Fetch Data.output.name))}}`.

### String

| Function | Notes |
|---|---|
| `upper(text)` | Uppercase. |
| `lower(text)` | Lowercase. |
| `trim(text)` | Strip leading and trailing whitespace. |
| `trim_start(text)` | Strip leading whitespace only. |
| `trim_end(text)` | Strip trailing whitespace only. |
| `len(text)` | Character count. If `text` is a JSON array, returns its element count instead. |
| `contains(text, substring)` | `true`/`false`. |
| `starts_with(text, prefix)` | `true`/`false`. |
| `ends_with(text, suffix)` | `true`/`false`. |
| `slice(text, start, end)` | Substring from `start` up to (not including) `end`, by character position. |
| `replace(text, find, replacement)` | Replaces the first match only. |
| `replace_all(text, find, replacement)` | Replaces every match. |
| `split(text, separator)` | Returns a JSON array of the pieces. |
| `join(array, separator)` | Takes a JSON array (as text) and joins its elements with `separator`. |
| `pad_start(text, length, char)` | Pads on the left to `length` characters using `char`. No-op if already at or past `length`. |
| `pad_end(text, length, char)` | Same, padding on the right. |

### Number

| Function | Notes |
|---|---|
| `round(number, decimals)` | `decimals` defaults to 0. |
| `floor(number)` | |
| `ceil(number)` | |
| `abs(number)` | |
| `min(a, b)` | |
| `max(a, b)` | |
| `to_int(number)` | Truncates a decimal value. |
| `to_float(number)` | |

### Date

All timestamps are ISO 8601. `now()` and `$run.timestamp` return the same kind of value; the difference is `now()` is a function you can pass into `format_date` and friends, while `$run.timestamp` is a value on its own.

| Function | Notes |
|---|---|
| `now()` | Current time. |
| `format_date(timestamp, format)` | `format` uses `YYYY`, `MM`, `DD`, `HH`, `mm`, `ss` as placeholders, e.g. `format_date($run.timestamp, "YYYY-MM-DD")`. Defaults to `YYYY-MM-DD` if `format` is omitted. |
| `parse_date(text)` | Parses a date string (ISO 8601, RFC 2822, or plain `YYYY-MM-DD`) into a normalized ISO timestamp. |
| `add_days(timestamp, n)` | `n` can be negative. |
| `add_hours(timestamp, n)` | |
| `add_minutes(timestamp, n)` | |
| `date_diff(timestamp_a, timestamp_b, unit)` | `unit` is `days`, `hours`, `minutes`, or `seconds`. |

### Array and object

These take a JSON array or object as their first argument, either a literal or a resolved `{{...}}` reference.

| Function | Notes |
|---|---|
| `first(array)` | First element. |
| `last(array)` | Last element. |
| `nth(array, index)` | Zero-based. |
| `keys(object)` | Returns a JSON array of key names. |
| `values(object)` | Returns a JSON array of values. |

### Conditional

| Function | Notes |
|---|---|
| `if(condition, then, else)` | Returns `then` when `condition` is "truthy," `else` otherwise. Truthy means non-empty and not `false`, `0`, or `null`. |

## The If / Condition node is different

The If / Condition node's Condition field (see [Nodes Reference](nodes.md), Logic section) looks like an expression, but it isn't evaluated by the function system above. Aerini resolves any `{{...}}` inside it first, the same as any other field, and only after that does a separate, much smaller comparison step look at the resulting plain text.

That comparison step understands:

- Numeric and string comparisons: `>`, `<`, `>=`, `<=`, `==`, `!=` (and bare `=` as another spelling of `==`). If both sides parse as numbers, it compares numerically; otherwise it compares the text case-insensitively. The condition is split at the first operator it finds, so the left side should not itself contain `>`, `<`, `=` or `!`.
- `contains`: `{{Webhook.output.body.tags}} contains urgent`. Case-insensitive, and checked before the symbol operators, so a left side such as `a=b contains b` works. An empty left or right side is treated as empty text. The left side must not itself contain the word `contains` surrounded by spaces.
- A bare value with no operator: `true`, `yes`, and `1` count as true; `false`, `no`, `0`, `null`, and an empty string count as false; anything else (including the literal text `undefined`) counts as true.

So `{{HTTP Request.output.status}} == 200` first resolves to something like `200 == 200`, and only then gets compared. Write the condition as if you're describing the comparison in plain terms; you don't need `if()` or any of the functions above inside this field.

## Testing a node

The **Test** button in a node's settings, and **Run this node** in the node's menu, run the node together with the steps connected before it, so references like `{{Fetch.output.body}}` have real data. If those steps were already run and nothing before them has changed, their last results are reused instead of running them again; the settings panel says which ones and how old they are, and **Run everything** forces a fresh run. If a step that has to run again would send an email or message, change data elsewhere or use your AI account, Test lists it and asks first. Loops always run again. Reused results are kept in memory only and are dropped when you switch workflows.

Number, true-or-false and dropdown fields in a node's settings have a **Use data** switch. Turn it on to fill the field from an earlier step (type `{{` or use **Insert data**); turn it off to return to your fixed value. When the whole field is one reference, the engine converts the result to a number or true/false before the node runs (`true` and `false` in any case; a whole number for integer fields). If the text can't be converted, the node's default is used and the run log warns. Mixed text such as `x{{...}}` stays text. Safety switches (names starting `allow_`) and fields that change a node's ports or layout don't have the switch.

Every plain reference in a text field (`{{Node.output.field}}`) also shows as a chip under the field, with the node icon, the node name and the field path. Click a chip to find the node and open its field list; press Backspace or Delete on a focused chip, or use its remove button, to delete that reference from the text. The field still holds the plain `{{...}}` text, so you can edit it by hand at any time.

You can also drag a wire from an earlier node's result dot onto a text field while the node's settings are open. Fields that can take that node's data are outlined; letting go inserts its main result at the end of the field, or opens the field list for that node if you hold Shift or the node has no single main result. Esc or letting go elsewhere cancels and does not create a wire.

### Describe it

Text fields have a **Describe it** button next to **Insert data**. Type what the field should contain, for example "the summary, then the article title in brackets", and press **Propose**. The panel shows exactly what will be sent before anything is sent. By default that is only the names and types of the fields in earlier nodes, never their values; tick **Include sample values** to add values from the last run. If the model is on the internet the panel says so and says which values would be sent; a model on your own computer (such as Ollama) keeps everything local. Secrets and saved keys are never part of the request.

The proposal is checked before you see it: every `{{...}}` reference must be a field that exists in an earlier node, and any other reference is left out and listed. You see the proposed text and, after a run, what it would resolve to. **Use** puts it in the field (Undo reverts it); **Cancel** changes nothing. Describe it uses an AI Prompt or AI Agent node on the canvas that has a saved connection, or that points at a local model; with none, the button's panel explains this and links to Credentials.

## Gotchas

- **The Database node's `query` field refuses `{{...}}` entirely.** Values get pasted directly into the SQL string, so there's no safe way to sanitize a numeric injection there. Use `?` placeholders in `query` and put the actual values in the `params` field instead. See [Database](nodes.md#database).
- **Function arguments have a nesting limit of 64 levels**, and a node's config values have a separate structural nesting limit, also 64. Both exist to stop a runaway or malicious `{{...}}` from crashing the engine; past the limit, the expression resolves to empty string with a warning instead. You will not run into either limit through normal use.
- **Inside a quoted function argument, `\"`, `\'`, and `\\` are unescaped** to a literal `"`, `'`, and `\`. So `{{upper("say \"hi\"")}}` resolves to `SAY "HI"`, not `SAY \"HI\"`.

## What's next

- [Nodes Reference](nodes.md): every node whose fields these expressions can go into.
- [Concepts](../getting-started/concepts.md): the mental model this page assumes.
- [Glossary](../glossary.md): quick lookup for any term on this page.
