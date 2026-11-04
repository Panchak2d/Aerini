import { cancelRun, runWorkflow, type WorkflowResult } from "../ipc/workflow";
import { valueAt, type SourceGraph } from "../data-sources";
import { previewField } from "../field-preview";
import {
  DESCRIBE_NODE_ID,
  DESCRIBE_REQUEST_MAX,
  DESCRIBE_TIMEOUT_MS,
  buildDescribeContext,
  buildDescribeWorkflow,
  describeFailure,
  describePromptText,
  describeSystemText,
  listDescribeProviders,
  parseProposal,
  providerNotice,
  type DescribeProvider,
  type DescribeSourceNode,
  type Proposal,
} from "../describe-field";

type Field = HTMLInputElement | HTMLTextAreaElement;

interface DescribeNode {
  data: DescribeSourceNode["data"] & {
    ports: unknown;
    input_schema: unknown;
    output_schema?: Record<string, unknown>;
  };
}

export interface DescribeGraph {
  connectors: SourceGraph["connectors"];
  nodes: Map<string, DescribeNode>;
}

const SPARKLE_SVG = `<svg width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><path d="M12 3l1.8 5.2L19 10l-5.2 1.8L12 17l-1.8-5.2L5 10l5.2-1.8z"/><path d="M19 16l.7 2.1L22 19l-2.3.9L19 22l-.7-2.1L16 19l2.3-.9z"/></svg>`;

function el<K extends keyof HTMLElementTagNameMap>(tag: K, cls: string, text?: string): HTMLElementTagNameMap[K] {
  const e = document.createElement(tag);
  e.className = cls;
  if (text !== undefined) e.textContent = text;
  return e;
}

/** Adds the "Describe it" button to a field's bar and the panel under it. */
export function mountDescribeIt(
  field: Field,
  host: HTMLElement,
  bar: HTMLElement,
  nodeId: string,
  getGraph: () => DescribeGraph | undefined,
): void {
  const btn = el("button", "field-describe");
  btn.type = "button";
  btn.setAttribute("aria-label", "Describe what this field should contain");
  btn.title = "Describe what this field should contain and get a proposal that uses data from earlier nodes";
  btn.innerHTML = `${SPARKLE_SVG}<span>Describe it</span>`;
  bar.appendChild(btn);

  const panel = el("div", "field-describe-panel");
  panel.hidden = true;
  host.appendChild(panel);

  let runId: string | null = null;

  function close(): void {
    if (runId !== null) { cancelRun(runId).catch(() => {}); runId = null; }
    panel.hidden = true;
    panel.replaceChildren();
    btn.setAttribute("aria-expanded", "false");
  }

  function state() {
    const g = getGraph();
    const providers = g ? listDescribeProviders(g.nodes.values()) : [];
    const upstreamCount = g ? buildDescribeContext(g, nodeId, false).nodes.length : 0;
    return { g, providers, upstreamCount };
  }

  function renderBlocked(message: string, withCredentials: boolean): void {
    panel.replaceChildren();
    panel.appendChild(el("div", "field-describe-note", message));
    const row = el("div", "field-describe-actions");
    if (withCredentials) {
      const open = el("button", "field-describe-btn", "Open Credentials");
      open.type = "button";
      open.addEventListener("click", () => { document.getElementById("btn-credentials")?.click(); });
      row.appendChild(open);
    }
    const cancel = el("button", "field-describe-btn", "Close");
    cancel.type = "button";
    cancel.addEventListener("click", close);
    row.appendChild(cancel);
    panel.appendChild(row);
  }

  function render(): void {
    const { g, providers, upstreamCount } = state();
    if (!g) return;
    if (upstreamCount === 0) {
      renderBlocked("Connect a node to this one first. Then you can describe the field and its data is offered.", false);
      return;
    }
    if (providers.length === 0) {
      renderBlocked(
        "Describe it needs an AI model. Add an AI Prompt or AI Agent node with a saved connection in Credentials, or one that uses a local model such as Ollama, then try again.",
        true,
      );
      return;
    }

    panel.replaceChildren();
    panel.appendChild(el("div", "field-describe-title", "Describe what this field should contain"));

    const request = el("textarea", "field-describe-input");
    request.rows = 2;
    request.maxLength = DESCRIBE_REQUEST_MAX;
    request.placeholder = "For example: the summary, then the article title in brackets";
    request.setAttribute("aria-label", "Describe what this field should contain");
    panel.appendChild(request);

    let provider: DescribeProvider = providers[0];
    const select = el("select", "field-describe-select");
    select.setAttribute("aria-label", "AI model to use");
    for (const p of providers) {
      const o = document.createElement("option");
      o.value = p.nodeId;
      o.textContent = p.label;
      select.appendChild(o);
    }
    if (providers.length > 1) panel.appendChild(select);

    const notice = el("div", "field-describe-note");
    panel.appendChild(notice);

    const samplesLabel = el("label", "field-describe-check");
    const samples = document.createElement("input");
    samples.type = "checkbox";
    samplesLabel.appendChild(samples);
    const samplesText = el("span", "");
    samplesLabel.appendChild(samplesText);
    panel.appendChild(samplesLabel);

    const sent = el("details", "field-describe-sent");
    sent.appendChild(el("summary", "", "What will be sent"));
    const sentPre = el("pre", "field-describe-pre");
    sent.appendChild(sentPre);
    panel.appendChild(sent);
    sent.open = true;

    const status = el("div", "field-describe-status");
    status.setAttribute("aria-live", "polite");
    panel.appendChild(status);
    const result = el("div", "field-describe-result");
    panel.appendChild(result);

    const actions = el("div", "field-describe-actions");
    const propose = el("button", "field-describe-btn field-describe-primary", "Propose");
    propose.type = "button";
    const cancel = el("button", "field-describe-btn", "Cancel");
    cancel.type = "button";
    actions.append(propose, cancel);
    panel.appendChild(actions);

    function current() {
      return buildDescribeContext(g!, nodeId, samples.checked);
    }
    function refreshSent(): void {
      provider = providers.find((p) => p.nodeId === select.value) ?? providers[0];
      notice.textContent = providerNotice(provider);
      samplesText.textContent = provider.local
        ? "Include sample values from the last run (they stay on this computer)"
        : "Include sample values from the last run (these values are sent to your AI provider)";
      sentPre.textContent = `${describeSystemText()}\n\n${describePromptText(request.value || "(your description)", current())}`;
    }
    request.addEventListener("input", refreshSent);
    select.addEventListener("change", refreshSent);
    samples.addEventListener("change", refreshSent);
    refreshSent();

    cancel.addEventListener("click", close);
    panel.addEventListener("keydown", (e) => {
      if (e.key === "Escape") { e.stopPropagation(); close(); btn.focus(); }
    });

    propose.addEventListener("click", () => { void run(); });

    async function run(): Promise<void> {
      const text = request.value.trim();
      if (text === "") { status.textContent = "Describe what the field should contain first."; return; }
      const ctx = current();
      const src = g!.nodes.get(provider.nodeId);
      if (!src) { status.textContent = "That AI node is no longer on the canvas."; return; }
      result.replaceChildren();
      status.textContent = "Asking the model...";
      propose.disabled = true;
      const myRun = `describe_${Date.now()}_${Math.random().toString(36).slice(2, 7)}`;
      runId = myRun;
      const json = buildDescribeWorkflow(
        provider,
        { ports: src.data.ports, input_schema: src.data.input_schema, output_schema: src.data.output_schema },
        describeSystemText(),
        describePromptText(text, ctx),
        `describe_${nodeId}`,
      );
      let timer: ReturnType<typeof setTimeout> | undefined;
      const timeout = new Promise<never>((_, reject) => {
        timer = setTimeout(() => {
          cancelRun(myRun).catch(() => {});
          reject(new Error("Timed out"));
        }, DESCRIBE_TIMEOUT_MS);
      });
      try {
        const res: WorkflowResult = await Promise.race([runWorkflow(json, {}, myRun), timeout]);
        if (runId !== myRun) return;
        if (!res.success) throw new Error(res.error ?? "The model could not answer.");
        const reply = valueAt(res.node_outputs[DESCRIBE_NODE_ID], "content");
        status.textContent = "";
        showProposal(parseProposal(reply, ctx.valid));
      } catch (err) {
        if (runId !== myRun) return;
        status.textContent = describeFailure(err);
        const retry = el("button", "field-describe-btn", "Retry");
        retry.type = "button";
        retry.addEventListener("click", () => { void run(); });
        result.replaceChildren(retry);
      } finally {
        clearTimeout(timer);
        if (runId === myRun) runId = null;
        propose.disabled = false;
      }
    }

    function showProposal(p: Proposal): void {
      result.replaceChildren();
      if (!p.ok) {
        status.textContent = p.error ?? "No usable proposal.";
        return;
      }
      result.appendChild(el("div", "field-describe-label", "Proposed text"));
      result.appendChild(el("pre", "field-describe-pre field-describe-proposal", p.text));
      const nodes = [...g!.nodes.values()].map((n) => ({ id: n.data.id, name: n.data.name }));
      const pv = previewField(p.text, nodes);
      const sample = el("div", "field-describe-sample");
      if (pv.status === "ok") {
        sample.textContent = `With the last run's data: ${pv.text.length > 200 ? pv.text.slice(0, 200) + "..." : pv.text}`;
      } else if (pv.status === "none") {
        sample.textContent = "This text has no data references.";
      } else if (pv.status === "empty") {
        sample.textContent = `The last run had nothing for ${pv.nodeName ?? "that node"}${pv.path ? `, ${pv.path}` : ""}.`;
      } else {
        sample.textContent = "Run the workflow once to see the resolved text.";
      }
      result.appendChild(sample);
      if (p.dropped.length > 0) {
        result.appendChild(el("div", "field-describe-note", `Left out data that does not exist: ${p.dropped.map((d) => `{{${d}}}`).join(", ")}`));
      }
      const row = el("div", "field-describe-actions");
      const use = el("button", "field-describe-btn field-describe-primary", "Use");
      use.type = "button";
      use.addEventListener("click", () => {
        field.value = p.text;
        field.selectionStart = field.selectionEnd = p.text.length;
        field.dispatchEvent(new Event("input", { bubbles: true }));
        close();
        field.focus();
      });
      const again = el("button", "field-describe-btn", "Try again");
      again.type = "button";
      again.addEventListener("click", () => { void run(); });
      const no = el("button", "field-describe-btn", "Cancel");
      no.type = "button";
      no.addEventListener("click", close);
      row.append(use, again, no);
      result.appendChild(row);
      use.focus();
    }

    request.focus();
  }

  btn.setAttribute("aria-expanded", "false");
  btn.addEventListener("click", (e) => {
    e.stopPropagation();
    if (!panel.hidden) { close(); return; }
    const { providers, upstreamCount } = state();
    btn.classList.toggle("field-describe--unavailable", providers.length === 0 || upstreamCount === 0);
    panel.hidden = false;
    btn.setAttribute("aria-expanded", "true");
    render();
  });
}
