import { CRON_PRESETS, mk, mkCustomSelect, mkField, type ExtensionContext } from "./popover-utils";

export function renderScheduleFields(ctx: ExtensionContext): void {
  const { node, body, onChange, rerender } = ctx;

  // Ensure a default mode is always set so the node is runnable immediately.
  if (!node.data.config["mode"]) {
    node.data.config["mode"] = "interval";
    onChange();
  }

  const schedModes = [
    { value: "interval", label: "Interval (seconds)" },
    { value: "cron",     label: "Cron Expression" },
    { value: "once",     label: "Run Once" },
  ];

  body.appendChild(mkField("Mode", () => {
    return mkCustomSelect(
      schedModes.map(m => m.label),
      schedModes.find(m => m.value === node.data.config["mode"])?.label ?? "Interval (seconds)",
      (label) => {
        const opt = schedModes.find(m => m.label === label);
        if (opt) {
          node.data.config["mode"] = opt.value;
          onChange();
          // Re-render so conditional fields update to match the new mode.
          rerender();
        }
      },
    );
  }, "When to trigger this workflow"));

  const currentMode = node.data.config["mode"] as string;

  if (currentMode === "interval") {
    body.appendChild(mkField("Interval (seconds)", () => {
      const inp = mk<HTMLInputElement>("input");
      inp.type = "number";
      inp.min = "10";
      inp.autocomplete = "off";
      inp.value = String(node.data.config["interval_secs"] ?? 60);
      inp.placeholder = "Seconds between runs";

      const warning = document.createElement("div");
      warning.className = "field-hint field-hint--warn";
      warning.style.display = "none";
      warning.textContent = "Minimum is 10 seconds — value reset to 10.";

      inp.addEventListener("input", () => {
        node.data.config["interval_secs"] = Number(inp.value);
        onChange();
      });
      inp.addEventListener("blur", () => {
        const v = Number(inp.value);
        if (!Number.isNaN(v) && v < 10) {
          inp.value = "10";
          node.data.config["interval_secs"] = 10;
          onChange();
          warning.style.display = "block";
          setTimeout(() => { warning.style.display = "none"; }, 3000);
        }
      });

      const wrap = document.createElement("div");
      wrap.appendChild(inp);
      wrap.appendChild(warning);
      return wrap;
    }, "Seconds between runs (minimum 10)"));
  }

  if (currentMode === "cron") {
    body.appendChild(mkField("Cron Expression", () => {
      const wrap = document.createElement("div");
      wrap.className = "cron-wrap";

      const presetsRow = document.createElement("div");
      presetsRow.className = "cron-presets";

      // inp is referenced in preset click handlers below; hoisted via const.
      const inp = mk<HTMLInputElement>("input");

      CRON_PRESETS.forEach(p => {
        const btn = document.createElement("button");
        btn.type = "button";
        btn.className = "cron-preset-btn";
        btn.textContent = p.label;
        btn.title = p.value;
        btn.addEventListener("click", () => {
          inp.value = p.value;
          node.data.config["cron_expr"] = p.value;
          onChange();
        });
        presetsRow.appendChild(btn);
      });

      inp.value = String(node.data.config["cron_expr"] ?? "");
      inp.placeholder = "e.g. 0 9 * * 1-5";
      inp.autocomplete = "off";
      inp.spellcheck = false;
      inp.addEventListener("input", () => { node.data.config["cron_expr"] = inp.value; onChange(); });

      const hint = document.createElement("div");
      hint.className = "cron-hint";
      hint.textContent = "min hour day month weekday (0=Sun)";

      wrap.appendChild(presetsRow);
      wrap.appendChild(inp);
      wrap.appendChild(hint);
      return wrap;
    }, "Cron expression e.g. 0 9 * * 1-5 (weekdays at 9am)"));
  }

  if (currentMode === "once") {
    body.appendChild(mkField("Run At", () => {
      const inp = mk<HTMLInputElement>("input");
      inp.type = "text";
      inp.autocomplete = "off";
      inp.spellcheck = false;
      inp.placeholder = "ISO timestamp e.g. 2025-12-31T09:00:00Z";
      inp.addEventListener("input", () => { node.data.config["run_at"] = inp.value; onChange(); });
      return inp;
    }, "ISO 8601 timestamp — when to fire once"));
  }
}
