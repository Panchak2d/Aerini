# Expressions

Flowo uses `{{...}}` template syntax to wire node outputs into other nodes' inputs. Any string field in any node's config can contain one or more expressions.

## What is an expression?

When you type `{{code.output.message}}` into a node's text field, Flowo replaces that placeholder with the actual value when the workflow runs. Think of it like mail merge — you write the template once, and the real value is filled in at runtime.

You can mix static text and expressions freely:

```
Hello {{HTTP Request.output.body.user.name}}, your order is confirmed.
```

---

## Syntax

### Node output reference

```
{{node_name.output.field}}
```

`node_name` is the **exact display name** of the node as it appears on the canvas — including spaces, capitalisation, and special characters. If you have a node named "HTTP Request", reference it as `HTTP Request`:

```
{{HTTP Request.output.body}}
{{Slack.output.ts}}
{{code.output.result}}
```

The easiest way to get the correct expression is to press `{{` in any text field to open the autocomplete dropdown, which inserts the exact name for you. If you type the expression manually, the name must match exactly — the comparison is case-sensitive.

> **Tip:** Long or complex node names like "Code (JS)" are a pain to type. Rename any node by double-clicking its title on the canvas. The renamed name is what you use in expressions.

### Nested fields

Dot-path traversal works to any depth:

```
{{HTTP Request.output.body.user.email}}
{{HTTP Request.output.body.results[0].name}}
```

Array indexing uses `[N]` notation. `[0]` is the first element.

### Run context

These are available in every run, with no node name prefix:

| Expression | Value |
|---|---|
| `{{$run.id}}` | Unique execution ID (UUID) for this run |
| `{{$run.timestamp}}` | ISO 8601 timestamp of when the run started |
| `{{$run.workflow_name}}` | The workflow's display name |

### Environment variables

```
{{$env.VAR_NAME}}
```

Disabled in the desktop app. In server mode, opt-in per variable with `--allow-env-vars`. See [Server Deployment — Environment Variables](server-deploy.md#environment-variable-expressions).

---

## Inline functions

Wrap any expression in a function call to transform its value:

```
{{upper(HTTP Request.output.body.status)}}
{{format_date($run.timestamp, "YYYY-MM-DD")}}
{{round(price_node.output.total, 2)}}
```

Functions accept one expression argument, plus optional extra literal string or number arguments (quoted with `""`).

---

## Function reference

### String

| Function | What it does | Example |
|---|---|---|
| `upper(str)` | Convert to uppercase | `{{upper(node.output.name)}}` → `ALICE` |
| `lower(str)` | Convert to lowercase | `{{lower(node.output.name)}}` → `alice` |
| `trim(str)` | Strip leading and trailing whitespace | `{{trim(node.output.text)}}` |
| `trim_start(str)` | Strip leading whitespace only | `{{trim_start(node.output.text)}}` |
| `trim_end(str)` | Strip trailing whitespace only | `{{trim_end(node.output.text)}}` |
| `len(str\|array)` | Character count of a string, or element count of an array | `{{len(node.output.items)}}` |
| `contains(str, search)` | Returns `true` or `false` | `{{contains(node.output.body, "error")}}` |
| `starts_with(str, prefix)` | Returns `true` or `false` | `{{starts_with(node.output.path, "/api")}}` |
| `ends_with(str, suffix)` | Returns `true` or `false` | `{{ends_with(node.output.file, ".pdf")}}` |
| `slice(str, start, end)` | Extract a substring by character positions (0-based) | `{{slice(node.output.id, 0, 8)}}` |
| `replace(str, from, to)` | Replace the first occurrence of `from` with `to` | `{{replace(node.output.text, "foo", "bar")}}` |
| `replace_all(str, from, to)` | Replace every occurrence of `from` with `to` | `{{replace_all(node.output.text, " ", "_")}}` |
| `split(str, delimiter)` | Split a string into a JSON array | `{{split(node.output.csv_row, ",")}}` |
| `join(array, delimiter)` | Join an array into a string | `{{join(node.output.tags, ", ")}}` |
| `pad_start(str, length, char)` | Left-pad a string to a minimum length | `{{pad_start(node.output.id, 8, "0")}}` → `00000042` |
| `pad_end(str, length, char)` | Right-pad a string to a minimum length | `{{pad_end(node.output.code, 6, " ")}}` |

### Number

| Function | What it does | Example |
|---|---|---|
| `round(n, decimals?)` | Round to N decimal places (default `0`) | `{{round(node.output.price, 2)}}` → `9.99` |
| `floor(n)` | Round down to nearest integer | `{{floor(node.output.score)}}` |
| `ceil(n)` | Round up to nearest integer | `{{ceil(node.output.score)}}` |
| `abs(n)` | Absolute value (remove the minus sign) | `{{abs(node.output.delta)}}` |
| `min(a, b)` | Smaller of two values | `{{min(node.output.count, "100")}}` |
| `max(a, b)` | Larger of two values | `{{max(node.output.count, "1")}}` |
| `to_int(str)` | Parse a string as an integer | `{{to_int(node.output.quantity)}}` |
| `to_float(str)` | Parse a string as a decimal number | `{{to_float(node.output.rate)}}` |

### Date and time

All date functions work with ISO 8601 timestamps (e.g. `2025-03-01T09:00:00Z`). `$run.timestamp` is always in this format.

| Function | What it does | Example |
|---|---|---|
| `now()` | Current UTC time as an ISO 8601 string | `{{now()}}` |
| `format_date(ts, format)` | Format a timestamp using the tokens below | `{{format_date($run.timestamp, "YYYY-MM-DD")}}` → `2025-03-01` |
| `parse_date(str)` | Convert various date formats to ISO 8601 | `{{parse_date(node.output.date)}}` |
| `add_days(ts, n)` | Add N days to a timestamp (negative to subtract) | `{{add_days($run.timestamp, 7)}}` |
| `add_hours(ts, n)` | Add N hours | `{{add_hours($run.timestamp, -2)}}` |
| `add_minutes(ts, n)` | Add N minutes | `{{add_minutes($run.timestamp, 30)}}` |
| `date_diff(ts1, ts2, unit)` | Difference between two timestamps. Unit: `days`, `hours`, `minutes`, or `seconds`. Returns a signed integer (positive if ts1 is later than ts2). | `{{date_diff($run.timestamp, node.output.created_at, "hours")}}` |

**`format_date` format tokens:**

| Token | Output | Example |
|---|---|---|
| `YYYY` | 4-digit year | `2025` |
| `MM` | 2-digit month | `03` |
| `DD` | 2-digit day | `01` |
| `HH` | 2-digit hour (24h, UTC) | `09` |
| `mm` | 2-digit minute | `05` |
| `ss` | 2-digit second | `00` |

Tokens are case-sensitive. Combine them freely: `"DD/MM/YYYY"`, `"YYYY-MM-DD HH:mm"`, `"HH:mm:ss"`.

### Array and object

| Function | What it does | Example |
|---|---|---|
| `first(array)` | First element of an array | `{{first(node.output.items)}}` |
| `last(array)` | Last element of an array | `{{last(node.output.items)}}` |
| `nth(array, index)` | Element at position N (0-based) | `{{nth(node.output.items, 2)}}` — third item |
| `keys(object)` | Array of all key names in an object | `{{keys(node.output.body)}}` |
| `values(object)` | Array of all values in an object | `{{values(node.output.body)}}` |

### Conditional

| Function | What it does | Example |
|---|---|---|
| `if(condition, true_value, false_value)` | Return one of two values based on a condition. Falsy values: empty string, `"false"`, `"0"`, `"null"`. Everything else is truthy. | `{{if(node.output.success, "Done", "Failed")}}` |

---

## Fallback behavior

Any expression that can't be resolved produces an empty string and logs an `INFO`-level warning. It never errors the workflow — if you're getting unexpected empty values, check the **Logs** tab after a run to see which expressions didn't resolve.

---

## Examples

Combine static text and expressions in one field:

```
Hello {{HTTP Request.output.body.user.name}}, your order {{$run.id}} is confirmed.
```

Use array indexing to get the first forecast day:

```
{{Weather API.output.body.forecast.forecastday[0].day.maxtemp_c}}
```

Format a timestamp for a report header:

```
Report generated at {{format_date($run.timestamp, "YYYY-MM-DD HH:mm")}} for {{$run.workflow_name}}
```

Conditional message based on a result:

```
Status: {{if(payment_node.output.status, "Payment successful", "Payment failed")}}
```

Format a number as currency:

```
Total: ${{round(cart_node.output.total, 2)}}
```

Pad a numeric ID to 6 digits with leading zeros:

```
ORDER-{{pad_start(order_node.output.id, 6, "0")}}
```

---

## Expression autocomplete

In any text field that supports expressions, press `{{` to open the autocomplete dropdown. It shows all available node outputs from the current canvas, letting you navigate the output structure without memorizing field names. The dropdown also lists all available functions.

---

## Notes

- Node names are matched **case-sensitively** and must match the name shown on the canvas exactly. `{{HTTP Request.output.body}}` and `{{http request.output.body}}` are not the same.
- Avoid giving nodes names that contain a dot (`.`) — the resolver splits expressions on dots, so a node named `v2.0 API` would not resolve correctly. Use underscores or spaces instead.
- If two nodes have the same name, the expression resolves to the first one in execution order. Rename your nodes to avoid this.
- Expressions inside JSON values work — you can put `{{Node Name.output.field}}` inside a JSON object field in any config panel.
- The resolver returns the string unchanged when there's no `{{` in the template, so static strings have no overhead.
