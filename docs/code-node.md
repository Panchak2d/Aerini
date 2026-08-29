# Code (JS) Node

The Code node runs a JavaScript snippet you write, using a Node.js 18+ runtime bundled with Aerini. It's the escape hatch for logic that no built-in node covers: custom data transforms, conditional computations, parsing unusual formats, or anything else that's easier to write as code than to assemble from nodes.

---

## Requirements

None — the runtime ships inside Aerini. You don't need Node.js installed on your machine, and this node never looks at your system PATH.

If the bundled runtime is ever missing or corrupt (a broken install), the node returns a clear error telling you to reinstall or redeploy Aerini rather than a cryptic failure.

---

## The input/output API

Two globals are available in every script:

**`input`** — the output of the node directly wired into this Code node. Access its fields directly:

```js
const value = input.body.result;
const name = input.user.name;
```

**`context`** — outputs from all upstream nodes, keyed by node name. Useful when you need data from a node that isn't directly wired in:

```js
const raw = context["HTTP Request"].body;
const parsed = context["Transform"].result;
```

**`output(value)`** — call this to pass a value to downstream nodes. Whatever you pass here becomes the node's output:

```js
output({ transformed: value.toUpperCase() });
```

If you don't call `output()`, the node's output is `null`.

---

## Async support

Top-level `await` and async functions work without any wrapper:

```js
const res = await fetch('https://api.example.com/data');
const json = await res.json();
output(json);
```

---

## Available globals

All Node.js built-in modules are available — `fs`, `path`, `crypto`, `http`, `child_process`, etc. Native `fetch` is available from Node.js 18 onward.

**No npm packages.** The Code node doesn't manage a `node_modules` folder. For external packages, the two common approaches are:

- Call an API that wraps the functionality you need (via the HTTP Request node or `fetch` inside the Code node)
- Use the Shell Command node to run a script that has its own dependencies

---

## Examples

### Transform a list of records

```js
const items = input.rows;
output(items.map(row => ({
    id: row.id,
    name: row.name.trim(),
    price: (row.price_cents / 100).toFixed(2),
})));
```

### Calculate an average

```js
const temperatures = input.readings;
const avg = temperatures.reduce((a, b) => a + b, 0) / temperatures.length;
output({
    average_celsius: avg,
    average_fahrenheit: avg * 9/5 + 32
});
```

### Reformat a date

```js
const raw = input.timestamp; // e.g. "2024-03-15T10:30:00Z"
const date = new Date(raw);
output({
    date: date.toISOString().split('T')[0],
    readable: date.toLocaleDateString('en-GB', { dateStyle: 'long' }),
});
```

### Branch on a computed value

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

---

## Security

In desktop mode, the Code node runs with your user's full permissions. This is intentional — you're running your own code on your own machine. Aerini warns you before running any workflow that contains a Code node.

In server mode (`aerini-server`), the Code node is disabled by default. Enable it with `--allow-code` only after reviewing every workflow that uses it. Any API token holder can then execute arbitrary JavaScript on the host. See [Security](security.md) for the full implications.
