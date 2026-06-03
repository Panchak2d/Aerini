# Code (JS) Node

## Requirements

Node.js 18+ must be installed and on your PATH. It is only required if you use
this node — not a global prerequisite for Flowo.

If Node.js is not found when this node runs, Flowo returns a clear error with
installation instructions.

---

## The input/output API

```js
// `input` — the data output of the upstream node
const value = input.body.result;

// `output(value)` — call this to pass a value downstream
output({ transformed: value.toUpperCase() });

// If you do not call output(), the node outputs null.
```

---

## Async support

Top-level await and async functions work:

```js
const res = await fetch('https://api.example.com/data');
const json = await res.json();
output(json);
```

---

## Available globals

- All Node.js built-in modules (`fs`, `path`, `crypto`, `http`, etc.)
- Native `fetch` (Node.js 18+)
- No npm packages — for external packages, either:
  - Use the HTTP Request node to call an API that wraps the package, or
  - Use the Shell Command node to run a script that manages its own dependencies

---

## Security note (server mode)

The Code node has access to all Node.js built-in modules, including `fs`,
`child_process`, and `net`. In desktop mode this is intentional — you are
running your own code on your own machine.

In server/API mode (`flowo-server api`), the Code node is disabled by default.
Enabling it with `--allow-code` grants every write-token holder the ability to
read files and spawn processes on the host. See [security.md](security.md).

---

## Examples

### Transform JSON

```js
const items = input.rows;
output(items.map(row => ({
    id: row.id,
    name: row.name.trim(),
    price: (row.price_cents / 100).toFixed(2),
})));
```

### Compute a value

```js
const temperatures = input.readings;
const avg = temperatures.reduce((a, b) => a + b, 0) / temperatures.length;
output({ average_celsius: avg, average_fahrenheit: avg * 9/5 + 32 });
```

### Parse and reformat a date

```js
const raw = input.timestamp; // e.g. "2024-03-15T10:30:00Z"
const date = new Date(raw);
output({
    date: date.toISOString().split('T')[0],
    readable: date.toLocaleDateString('en-GB', { dateStyle: 'long' }),
});
```

### Conditional branching helper

```js
const score = parseFloat(input.score);
if (isNaN(score)) {
    output({ status: 'error', message: 'score is not a number' });
} else if (score >= 0.8) {
    output({ status: 'pass', score });
} else {
    output({ status: 'fail', score });
}
```
