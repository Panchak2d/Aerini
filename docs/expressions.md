# Expressions

Expressions let you take data produced by one node and use it in another node's configuration. Without them, each node would be an island — expressions are what turn a collection of isolated steps into a connected workflow.

---

## The basics

Anywhere a node config field accepts a text value, you can embed an expression using double curly braces:

```
{{node_name.output.field_name}}
```

When the workflow runs, Flowo replaces every `{{...}}` block with the actual value from the running context. If the expression can't be resolved — because the node hasn't run yet, or the field name is wrong — it resolves to an empty string and a warning is added to the run logs.

**Example:** An HTTP Request node named `weather` fetches a weather API. The response body contains `{ "temperature": 21 }`. In a downstream Slack node's text field, you write:

```
Current temperature: {{weather.output.body.temperature}}°C
```

When the workflow runs, Flowo replaces that with `Current temperature: 21°C`.

---

## Referencing node outputs

The standard path for referencing another node's output is:

```
{{NodeName.output.field}}
```

- `NodeName` is the node's name as shown on the canvas. Rename a node by double-clicking its title.
- `.output` accesses the node's output object.
- `.field` is the specific field you want.

Node names are matched case-insensitively.

### Dot paths and array indexing

You can navigate nested objects with dots and access array elements with brackets:

```
{{http.output.body.user.email}}
{{http.output.body.items[0].name}}
{{http.output.body.items[2].price}}
```

### If you rename a node

Renaming a node on the canvas after building expressions that reference it will break those expressions. They'll produce empty strings at runtime with a warning in the logs. Use the expression picker to rebuild them after renaming.

---

## The expression picker

Press `{{` inside any text input field to open the expression picker. It shows all values currently available in the workflow — the output fields from every upstream node. Click any value to insert it as an expression. This is the recommended way to build expressions; you don't need to remember field names or path syntax.

---

## Special variables

These are available in every workflow without referencing a specific node.

### `$run.*`

| Expression | Value |
|---|---|
| `{{$run.id}}` | The execution ID for this run (a UUID). |
| `{{$run.timestamp}}` | ISO 8601 timestamp of when this expression was evaluated. |
| `{{$run.workflow_name}}` | The name of the current workflow. |

### `$env.*`

Reads an environment variable from the system. Disabled by default.

```
{{$env.HOME}}
{{$env.APP_ENV}}
```

In the desktop app, `$env` is always disabled for security — expressions referencing `$env` resolve to empty strings.

In server mode, each variable must be explicitly whitelisted when starting the server:

```bash
flowo-server api --allow-env-vars HOME,APP_ENV --token mytoken
```

Variables not on the allowlist resolve to empty strings. Never add `FLOWO_TOKEN` or `FLOWO_CRED_*` variables to the allowlist — doing so lets any workflow read and exfiltrate credentials.

---

## Functions

Call functions inside `{{...}}` to transform values before using them.

Syntax: `{{function(argument)}}` or `{{function(node.output.field, "literal")}}`

Arguments can be other expressions or quoted string literals.

### String functions

| Function | Example | Result |
|---|---|---|
| `upper(s)` | `{{upper(name.output.text)}}` | Converts to uppercase |
| `lower(s)` | `{{lower(name.output.text)}}` | Converts to lowercase |
| `trim(s)` | `{{trim(name.output.text)}}` | Removes leading and trailing whitespace |
| `trim_start(s)` | `{{trim_start(s)}}` | Removes leading whitespace only |
| `trim_end(s)` | `{{trim_end(s)}}` | Removes trailing whitespace only |
| `len(s)` | `{{len(name.output.items)}}` | Character count for strings, element count for arrays |
| `contains(s, sub)` | `{{contains(name.output.body, "error")}}` | `true` or `false` |
| `starts_with(s, prefix)` | `{{starts_with(name.output.url, "https")}}` | `true` or `false` |
| `ends_with(s, suffix)` | `{{ends_with(name.output.file, ".pdf")}}` | `true` or `false` |
| `slice(s, start, end)` | `{{slice(name.output.text, 0, 10)}}` | Substring by character position |
| `replace(s, from, to)` | `{{replace(name.output.text, "foo", "bar")}}` | Replaces first occurrence |
| `replace_all(s, from, to)` | `{{replace_all(name.output.text, " ", "_")}}` | Replaces all occurrences |
| `split(s, sep)` | `{{split(name.output.csv, ",")}}` | Returns a JSON array string |
| `join(arr, sep)` | `{{join(name.output.items, ", ")}}` | Joins array elements |
| `pad_start(s, len, char)` | `{{pad_start(name.output.id, 5, "0")}}` | Pads left to length |
| `pad_end(s, len, char)` | `{{pad_end(name.output.code, 8, " ")}}` | Pads right to length |

### Number functions

| Function | Example | Result |
|---|---|---|
| `round(n, decimals)` | `{{round(name.output.price, 2)}}` | Rounds to given decimal places |
| `floor(n)` | `{{floor(name.output.value)}}` | Rounds down |
| `ceil(n)` | `{{ceil(name.output.value)}}` | Rounds up |
| `abs(n)` | `{{abs(name.output.diff)}}` | Absolute value |
| `min(a, b)` | `{{min(name.output.a, name.output.b)}}` | Smaller of two numbers |
| `max(a, b)` | `{{max(name.output.a, name.output.b)}}` | Larger of two numbers |
| `to_int(s)` | `{{to_int(name.output.count)}}` | Converts string to integer |
| `to_float(s)` | `{{to_float(name.output.price)}}` | Converts string to float |

### Date functions

All date functions use ISO 8601 timestamps (`2024-01-15T09:00:00Z`).

| Function | Example | Result |
|---|---|---|
| `now()` | `{{now()}}` | Current UTC time as ISO 8601 |
| `format_date(ts, fmt)` | `{{format_date(name.output.ts, "YYYY-MM-DD")}}` | Formats a timestamp |
| `parse_date(s)` | `{{parse_date(name.output.date_str)}}` | Parses a date string to ISO 8601 |
| `add_days(ts, n)` | `{{add_days(now(), 7)}}` | Adds days to a timestamp |
| `add_hours(ts, n)` | `{{add_hours(now(), 2)}}` | Adds hours |
| `add_minutes(ts, n)` | `{{add_minutes(now(), 30)}}` | Adds minutes |
| `date_diff(ts1, ts2, unit)` | `{{date_diff(now(), name.output.created_at, "days")}}` | Difference between two timestamps |

Format tokens for `format_date`: `YYYY` (4-digit year), `MM` (month), `DD` (day), `HH` (hour), `mm` (minute), `ss` (second).

Units for `date_diff`: `days`, `hours`, `minutes`, `seconds`.

### Array / Object functions

| Function | Example | Result |
|---|---|---|
| `first(arr)` | `{{first(name.output.items)}}` | First element of a JSON array |
| `last(arr)` | `{{last(name.output.items)}}` | Last element of a JSON array |
| `nth(arr, n)` | `{{nth(name.output.items, 2)}}` | Element at index n (0-based) |
| `keys(obj)` | `{{keys(name.output.data)}}` | Array of object keys |
| `values(obj)` | `{{values(name.output.data)}}` | Array of object values |

### Conditional function

| Function | Example | Result |
|---|---|---|
| `if(condition, then, else)` | `{{if(name.output.status, "OK", "FAIL")}}` | Returns `then` if condition is truthy, else `else` |

Truthy values: any non-empty string except `"false"`, `"0"`, and `"null"`.

---

## Mixing expressions and literal text

Expressions can be embedded anywhere inside a string. The rest of the text is kept as-is:

```
Hello {{user.output.name}}, your order #{{order.output.id}} shipped on {{format_date(order.output.shipped_at, "YYYY-MM-DD")}}.
```

Multiple expressions in one field all resolve independently.

---

## How values are converted

All expression results are converted to strings before being inserted into text fields. Objects and arrays become their JSON representation. `null` and missing values become an empty string.

If a field accepts a non-string type (a number field, a boolean field), the resolved string is converted back to the expected type. For example, a number field containing `{{loop.output.index}}` receives the integer value, not a string.

---

## Warnings vs errors

Expression errors are warnings, not fatal errors. If an expression can't be resolved, the run continues with an empty string for that value, and a warning is written to the run logs. Check the Logs tab after a run to see any unresolved expressions.

If you see unexpected empty values in a downstream node's output, check the Logs tab — a warning about an unresolved expression is usually the cause.

---

## Using variables across runs

Expressions only reference data from the current run. To pass a value between runs (for example, a counter that increments each time the workflow runs), use Set Variable with `persist: true` and Get Variable.

---

## Practical examples

**Build a filename with a timestamp:**
```
report-{{format_date(now(), "YYYY-MM-DD")}}.csv
```

**Check the temperature from a weather API:**
In an If / Condition node's `condition` field:
```
{{weather.output.body.current.temperature_2m}} > 25
```

**Extract the first item from a results array:**
```
{{first(search.output.body.results)}}
```

**Format a price with 2 decimal places:**
```
${{round(cart.output.total, 2)}}
```

**Use the loop index in a filename:**
```
image-{{pad_start(loop.output.index, 3, "0")}}.png
```
