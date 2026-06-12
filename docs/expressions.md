# Expressions

Nodes are isolated by default — an HTTP Request node fetches data, but the next node has no idea what it returned unless you explicitly wire it in. Expressions are that wire.

Write `{{node_name.output.some_field}}` in any text field, and Flowo replaces it with the actual value at runtime. The syntax looks technical, but you rarely need to type it from scratch: press `{{` in any input field to open the expression picker, which shows every available value from upstream nodes and lets you click to insert.

---

## The basics

The standard pattern:

```
{{node_name.output.field}}
```

- `node_name` — the node's name as shown on the canvas. Rename any node by double-clicking its title. Names are matched case-insensitively.
- `.output` — accesses the node's result object.
- `.field` — the specific piece of data you want.

**Example:** An HTTP Request node named `weather` returns `{ "temperature": 21 }`. In a downstream Slack message field:

```
Current temperature: {{weather.output.body.temperature}}°C
```

At runtime, Flowo resolves that to `Current temperature: 21°C`.

Expressions can appear anywhere inside a string — mix them freely with literal text:

```
Hello {{user.output.name}}, your order #{{order.output.id}} shipped on {{format_date(order.output.shipped_at, "YYYY-MM-DD")}}.
```

---

## Navigating nested data

Use dots to drill into nested objects, and brackets for array indexes:

```
{{http.output.body.user.email}}
{{http.output.body.items[0].name}}
{{http.output.body.items[2].price}}
```

If you're not sure of the exact path, run the workflow once, open the output drawer, and look at the **Results** tab. It shows the full JSON output of every node — you can click through the tree to find the field you want, then use the expression picker to insert the correct path.

---

## The expression picker

Press `{{` inside any text input to open the picker. It lists every value available from nodes that run before the current one. Click any entry to insert it as a properly formatted expression.

Use the picker rather than typing paths manually. It's faster, it shows you what's actually available, and it prevents typos that produce silent empty values.

---

## Special variables

These are available in every workflow without referencing a specific node.

### `$run.*`

| Expression | What it returns |
|---|---|
| `{{$run.id}}` | A unique ID for this execution (a UUID) |
| `{{$run.timestamp}}` | ISO 8601 timestamp of when the expression was evaluated |
| `{{$run.workflow_name}}` | The name of the current workflow |

### `$env.*`

Reads an environment variable. **Disabled by default.**

```
{{$env.APP_ENV}}
```

In the desktop app, `$env` is always disabled — expressions referencing it resolve to empty strings.

In server mode, you must explicitly whitelist each variable name when starting the server:

```bash
flowo-server api --allow-env-vars HOME,APP_ENV --token mytoken
```

Variables not on that list return empty strings. Never add `FLOWO_TOKEN` or any `FLOWO_CRED_*` variable to the allowlist — a workflow could read and send your credentials to an external server.

---

## Functions

Call functions inside `{{...}}` to transform values before using them:

```
{{upper(user.output.name)}}
{{format_date(now(), "YYYY-MM-DD")}}
{{round(cart.output.total, 2)}}
```

Arguments can be other expressions or quoted string literals.

### String functions

| Function | Example | Result |
|---|---|---|
| `upper(s)` | `{{upper(name.output.text)}}` | UPPERCASE |
| `lower(s)` | `{{lower(name.output.text)}}` | lowercase |
| `trim(s)` | `{{trim(name.output.text)}}` | Strips leading and trailing whitespace |
| `trim_start(s)` | `{{trim_start(s)}}` | Strips leading whitespace only |
| `trim_end(s)` | `{{trim_end(s)}}` | Strips trailing whitespace only |
| `len(s)` | `{{len(name.output.items)}}` | Character count for strings, element count for arrays |
| `contains(s, sub)` | `{{contains(name.output.body, "error")}}` | `true` or `false` |
| `starts_with(s, prefix)` | `{{starts_with(name.output.url, "https")}}` | `true` or `false` |
| `ends_with(s, suffix)` | `{{ends_with(name.output.file, ".pdf")}}` | `true` or `false` |
| `slice(s, start, end)` | `{{slice(name.output.text, 0, 10)}}` | Substring by character position |
| `replace(s, from, to)` | `{{replace(name.output.text, "foo", "bar")}}` | Replaces first match |
| `replace_all(s, from, to)` | `{{replace_all(name.output.text, " ", "_")}}` | Replaces all matches |
| `split(s, sep)` | `{{split(name.output.csv, ",")}}` | Returns a JSON array string |
| `join(arr, sep)` | `{{join(name.output.items, ", ")}}` | Joins array elements into a string |
| `pad_start(s, len, char)` | `{{pad_start(name.output.id, 5, "0")}}` | Pads left to a given length |
| `pad_end(s, len, char)` | `{{pad_end(name.output.code, 8, " ")}}` | Pads right to a given length |

### Number functions

| Function | Example | Result |
|---|---|---|
| `round(n, decimals)` | `{{round(name.output.price, 2)}}` | Rounds to given decimal places |
| `floor(n)` | `{{floor(name.output.value)}}` | Rounds down |
| `ceil(n)` | `{{ceil(name.output.value)}}` | Rounds up |
| `abs(n)` | `{{abs(name.output.diff)}}` | Absolute value |
| `min(a, b)` | `{{min(name.output.a, name.output.b)}}` | Smaller of two values |
| `max(a, b)` | `{{max(name.output.a, name.output.b)}}` | Larger of two values |
| `to_int(s)` | `{{to_int(name.output.count)}}` | Converts string to integer |
| `to_float(s)` | `{{to_float(name.output.price)}}` | Converts string to float |

### Date functions

All date functions use ISO 8601 timestamps (`2024-01-15T09:00:00Z`).

| Function | Example | Result |
|---|---|---|
| `now()` | `{{now()}}` | Current UTC time as ISO 8601 |
| `format_date(ts, fmt)` | `{{format_date(name.output.ts, "YYYY-MM-DD")}}` | Formatted date string |
| `parse_date(s)` | `{{parse_date(name.output.date_str)}}` | Parses a date string to ISO 8601 |
| `add_days(ts, n)` | `{{add_days(now(), 7)}}` | Adds N days to a timestamp |
| `add_hours(ts, n)` | `{{add_hours(now(), 2)}}` | Adds N hours |
| `add_minutes(ts, n)` | `{{add_minutes(now(), 30)}}` | Adds N minutes |
| `date_diff(ts1, ts2, unit)` | `{{date_diff(now(), name.output.created_at, "days")}}` | Difference between two timestamps |

Format tokens for `format_date`: `YYYY` (4-digit year), `MM` (month 01–12), `DD` (day 01–31), `HH` (hour 00–23), `mm` (minute 00–59), `ss` (second 00–59).

Valid units for `date_diff`: `days`, `hours`, `minutes`, `seconds`.

### Array and object functions

| Function | Example | Result |
|---|---|---|
| `first(arr)` | `{{first(name.output.items)}}` | First element of a JSON array |
| `last(arr)` | `{{last(name.output.items)}}` | Last element |
| `nth(arr, n)` | `{{nth(name.output.items, 2)}}` | Element at index n (0-based) |
| `keys(obj)` | `{{keys(name.output.data)}}` | Array of object key names |
| `values(obj)` | `{{values(name.output.data)}}` | Array of object values |

### Conditional function

```
{{if(condition, then, else)}}
```

Returns `then` if the condition is truthy, otherwise `else`. Truthy means any non-empty string except `"false"`, `"0"`, and `"null"`.

Example:
```
{{if(order.output.status, "Confirmed", "Pending")}}
```

---

## How types work

All expression results are converted to strings before being inserted into text fields. Objects and arrays become their JSON representation. `null` and missing values become an empty string.

Fields that expect a specific type (a number field, a boolean toggle) receive the converted value back: a number field containing `{{loop.output.index}}` receives an integer, not a string. Flowo handles this conversion automatically.

---

## Passing values between runs

Expressions only see data from the current run. To carry a value from one run to the next — a counter, a cursor, a timestamp — use the **Set Variable** node with `persist: true` to store it, and the **Get Variable** node to read it back in a future run.

---

## Practical examples

**Build a filename with today's date:**
```
report-{{format_date(now(), "YYYY-MM-DD")}}.csv
```

**Check a numeric condition in an If / Condition node:**
```
{{weather.output.body.current.temperature_2m}} > 25
```

**Extract the first result from an array:**
```
{{first(search.output.body.results)}}
```

**Format a dollar amount:**
```
${{round(cart.output.total, 2)}}
```

**Zero-pad a loop index for filenames:**
```
image-{{pad_start(loop.output.index, 3, "0")}}.png
```

---

## Troubleshooting

**A field shows up blank when the workflow runs.**
Open the **Logs** tab in the output drawer. Flowo logs a warning for every unresolved expression, including the exact path it couldn't find. The most common cause is a typo in the node name or field path.

**I renamed a node and now expressions break.**
Renaming a node invalidates any expression that references it by the old name. Use the expression picker to rebuild the broken expressions with the new name.

**The expression picker shows nothing.**
The picker only lists outputs from nodes that are upstream (connected before) the current node in the workflow graph. If no nodes are connected before the one you're editing, the picker will be empty.

**An expression works on the first run but fails later.**
The upstream node may have returned a different shape on a subsequent run — an API that returned `null` instead of an object, for example. Check the Results tab to see what the upstream node actually produced on the failing run.
