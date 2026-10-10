import { mkField, mkCustomSelect, type ExtensionContext } from "../../node-configs/popover-utils";

export function renderHttpAuthMode(ctx: ExtensionContext): void {
  ctx.body.appendChild(mkField("Auth Mode", () => {
    const modes = [
      { value: "none",           label: "None" },
      { value: "bearer",         label: "Bearer Token" },
      { value: "api_key_header", label: "API Key Header" },
      { value: "basic",          label: "Basic Auth" },
    ];
    const cur = (ctx.node.data.config["auth_mode"] as string) ?? "bearer";
    return mkCustomSelect(
      modes.map(m => m.label),
      modes.find(m => m.value === cur)?.label ?? "Bearer Token",
      (label) => {
        const opt = modes.find(m => m.label === label);
        if (opt) { ctx.node.data.config["auth_mode"] = opt.value; ctx.onChange(); }
      },
    );
  }, "How the credential is attached to the request"));
}

export function renderHttpSsrfWarning(ctx: ExtensionContext): void {
  const note = document.createElement("div");
  note.className = "popover-info-banner";
  note.innerHTML =
    `<strong>SSRF protection:</strong> Requests to private, loopback, link-local, and cloud ` +
    `metadata addresses (e.g. <code>169.254.169.254</code>) are blocked, including when a ` +
    `hostname resolves to one, and redirects are not followed. ` +
    `See <strong>docs/guide/security.md</strong> for details.`;
  ctx.body.appendChild(note);
}
