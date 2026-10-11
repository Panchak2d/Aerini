# Expressions

An **[expression](../glossary.md#expression)** is the `{{...}}` syntax you type into a node's settings to pull in a value from somewhere else, instead of typing a fixed one. Type `{{HTTP Request.output.body}}` into a Send Email node's Body field, and that field sends whatever the HTTP Request node's response body actually was on that run, not a string you typed ahead of time.

This page covers what you can put inside `{{...}}`: node outputs, a handful of special variables, and a small set of built-in functions. For which config fields accept expressions at all, see the note at the top of [Nodes Reference](nodes.md).

## How it works

Aerini resolves every `{{...}}` in a node's config right before that node runs, using whatever data exists at that point in the workflow: earlier nodes' outputs, workflow variables set so far, and the current run's own metadata.

Only text fields carry expressions. A number field or a dropdown just has nowhere to type `{{`, so those always take a fixed value.

**An expression that can't be resolved never crashes the workflow.** It resolves to an empty string, and a warning explaining why lands in that run's log, things like "no node named 'X' in this workflow" or "field path not found." Check the run's log first if a field came out blank when you expected a value.

You can mix expressions with plain text in the same field: `Order {{HTTP Request.output.order_id}} shipped` resolves to `Order 4521 shipped`. A field can also contain several expressions at once.

## Referencing another node's output

The general shape is:

```
{{Node Name.output}}
{{Node Name.output.field}}
{{Node Name.output.field.nested}}
{{Node Name.output.array_field[0]}}
```

**Node Name** is the name you gave the node on the canvas (its display label), not any internal ID. If you rename a node, update anything referencing it by its old name.

- `{{Node Name.output}}` on its own returns the node's entire output, stringified. For a simple value that's just the value; for an object or array, you get its JSON text.
- Add a dot-path after `.output` to reach one field: `{{Fetch Data.output.body.name}}`.
- Add `[N]` to index into an array at any point in the path: `{{List Items.output.items[0]}}`, or `{{List Items.output.items[0].tags[1]}}` for a nested one.

A few things worth knowing about node references specifically:

- **If two nodes share the same name**, the expression resolves against whichever one comes first and logs a warning about the ambiguity. Give nodes distinct names rather than relying on this.
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

## Gotchas

- **The Database node's `query` field refuses `{{...}}` entirely.** Values get pasted directly into the SQL string, so there's no safe way to sanitize a numeric injection there. Use `?` placeholders in `query` and put the actual values in the `params` field instead. See [Database](nodes.md#database).
- **Function arguments have a nesting limit of 64 levels**, and a node's config values have a separate structural nesting limit, also 64. Both exist to stop a runaway or malicious `{{...}}` from crashing the engine; past the limit, the expression resolves to empty string with a warning instead. You will not run into either limit through normal use.
- **Inside a quoted function argument, `\"`, `\'`, and `\\` are unescaped** to a literal `"`, `'`, and `\`. So `{{upper("say \"hi\"")}}` resolves to `SAY "HI"`, not `SAY \"HI\"`.

## What's next

- [Nodes Reference](nodes.md): every node whose fields these expressions can go into.
- [Concepts](../getting-started/concepts.md): the mental model this page assumes.
- [Glossary](../glossary.md): quick lookup for any term on this page.
