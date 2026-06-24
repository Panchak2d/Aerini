import { CRON_PRESETS, mk, mkCustomSelect, mkField, type ExtensionContext } from "./popover-utils";

// Converts a stored ISO string (2025-12-31T09:00:00Z) to datetime-local format
// (2025-12-31T09:00) and back. datetime-local doesn't support seconds/timezone,
// so we store as UTC ISO and display truncated to the minute.
function isoToLocal(iso: string): string {
  return iso.replace(/:[0-9]{2}(\.[0-9]+)?(Z|[+-].+)?$/, "");
}
function localToIso(local: string): string {
  return local ? `${local}:00Z` : "";
}

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
      warning.setAttribute("role", "alert");
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
      presetsRow.setAttribute("role", "group");
      presetsRow.setAttribute("aria-label", "Quick presets");

      // inp is referenced in preset click handlers; assigned before any click can occur.
      const inp = mk<HTMLInputElement>("input");
      const currentCron = String(node.data.config["cron_expr"] ?? "");

      CRON_PRESETS.forEach(p => {
        const btn = document.createElement("button");
        btn.type = "button";
        btn.className = "cron-preset-btn";
        btn.textContent = p.label;
        btn.title = p.value;
        btn.setAttribute("aria-label", `${p.label} (${p.value})`);
        const isActive = p.value === currentCron;
        if (isActive) btn.classList.add("active");

        btn.addEventListener("click", () => {
          inp.value = p.value;
          node.data.config["cron_expr"] = p.value;
          onChange();
          // Update active state on all preset buttons.
          presetsRow.querySelectorAll(".cron-preset-btn").forEach(b => {
            b.classList.toggle("active", b === btn);
          });
        });
        presetsRow.appendChild(btn);
      });

      inp.value = currentCron;
      inp.placeholder = "e.g. 0 9 * * 1-5";
      inp.autocomplete = "off";
      inp.spellcheck = false;
      inp.addEventListener("input", () => {
        node.data.config["cron_expr"] = inp.value;
        // Clear active state when user types manually.
        presetsRow.querySelectorAll(".cron-preset-btn").forEach(b => {
          b.classList.toggle("active", (b as HTMLButtonElement).title === inp.value);
        });
        onChange();
      });

      const hint = document.createElement("div");
      hint.className = "cron-hint";
      hint.textContent = "min hour day month weekday  (0 = Sun)";

      wrap.appendChild(presetsRow);
      wrap.appendChild(inp);
      wrap.appendChild(hint);
      return wrap;
    }, "Cron expression e.g. 0 9 * * 1-5 (weekdays at 9 am)"));
  }

  if (currentMode === "once") {
    body.appendChild(mkField("Run At", () => {
      const inp = mk<HTMLInputElement>("input");
      inp.type = "datetime-local";
      inp.autocomplete = "off";
      // Load existing value, converting from stored ISO to datetime-local format.
      const stored = node.data.config["run_at"] as string | undefined;
      inp.value = stored ? isoToLocal(stored) : "";
      inp.addEventListener("input", () => {
        node.data.config["run_at"] = localToIso(inp.value);
        onChange();
      });
      return inp;
    }, "Date and time to fire this workflow once (stored as UTC)."));
  }
}
