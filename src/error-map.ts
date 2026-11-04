export interface FriendlyError {
  explanation: string;
  fix: string;
}

const CODE_MAP: Record<string, FriendlyError> = {
  TOOLS_NOT_SUPPORTED: {
    explanation: "This model cannot use tools, and the Tools field is filled in.",
    fix: "Pick a model that supports tools, or clear the Tools field.",
  },
  INVALID_TOOLS: {
    explanation: "The Tools field is not a valid list of tools.",
    fix: "Give every tool a unique name, or use Edit as JSON to check the text.",
  },
  PERSIST_UNAVAILABLE: {
    explanation: "Only New needs a database to remember seen items, and none is available here.",
    fix: "Turn Only New off, or run the workflow where a database exists.",
  },
  STATE_ERROR: {
    explanation: "The list of seen items could not be read or saved.",
    fix: "Run again. If it keeps happening, turn Only New off for this node.",
  },
  NOT_A_FEED: {
    explanation: "The text is not a feed. It is often a web page or an error response.",
    fix: "Check the address, and that the node before this one returns the feed text.",
  },
  INPUT_TOO_LARGE: {
    explanation: "The feed is larger than the 10 MiB limit.",
    fix: "Use a smaller feed.",
  },
  MISSING_INPUT: {
    explanation: "The Text field is blank.",
    fix: "Fill in Text, or connect a node that outputs the feed text.",
  },
  MISSING_API_KEY: {
    explanation: "This node needs an API key and none was found.",
    fix: "Open the node, find the Connection section, and pick or add a credential.",
  },
  MISSING_TOKEN: {
    explanation: "This node needs an access token and none was found.",
    fix: "Open the node, find the Connection section, and pick or add a credential.",
  },
  INVALID_API_KEY: {
    explanation: "The service rejected the API key.",
    fix: "Check the key in Credentials, and that it has the permissions this node needs.",
  },
  RATE_LIMITED: {
    explanation: "The service is limiting how often you can call it.",
    fix: "Wait and run again, add a Delay node before this one, or run the workflow less often.",
  },
  UPSTREAM_UNAVAILABLE: {
    explanation: "The service is temporarily unavailable.",
    fix: "Try again in a moment.",
  },
  NETWORK_ERROR: {
    explanation: "Aerini could not reach the server.",
    fix: "Check your internet connection and the address in the node.",
  },
  SSRF_BLOCKED: {
    explanation: "Aerini blocked this address because it points at a private or restricted network.",
    fix: "Use a public address. The SSRF section of the Security guide lists what is blocked.",
  },
  INVALID_CONFIG: {
    explanation: "A setting holds a value this node cannot use.",
    fix: "Open the node and check the field named in the message.",
  },
  CONTENT_POLICY_VIOLATION: {
    explanation: "The service refused the request under its content policy.",
    fix: "Reword the prompt and try again.",
  },
  BAD_KEY: {
    explanation: "The AI service rejected the API key.",
    fix: "Check the key in Credentials, and that it has access to the model you picked.",
  },
  TIMEOUT: {
    explanation: "The step took longer than its time limit and was stopped.",
    fix: "Raise the timeout in the node, or make the work smaller.",
  },
  REFUSED: {
    explanation: "The model declined to answer this request.",
    fix: "Reword the prompt and try again, or try a different model.",
  },
  EMPTY_RESPONSE: {
    explanation: "The service answered with nothing.",
    fix: "Run again. If it keeps happening, change the prompt or the model.",
  },
  QUERY_NOT_READ_ONLY: {
    explanation: "The query changes data, and this node only runs read queries.",
    fix: "Use a query that only reads, such as SELECT.",
  },
  ROW_LIMIT_EXCEEDED: {
    explanation: "The query returned more rows than the limit allows.",
    fix: "Add a WHERE or LIMIT to the query to return fewer rows.",
  },
  UNKNOWN_OPERATION: {
    explanation: "The Operation setting holds a value this node does not know.",
    fix: "Open the node and pick an operation from the list.",
  },
  OAUTH_NO_REFRESH_TOKEN: {
    explanation: "The saved sign-in has no refresh token, so Aerini cannot renew it.",
    fix: "Open Credentials and sign in to the service again.",
  },
  SMTP_ERROR: {
    explanation: "The mail server did not accept the message.",
    fix: "Check the server address, port, user name and password in the node.",
  },
};

const MISSING_PREFIX = /^MISSING_([A-Z0-9]+(?:_[A-Z0-9]+)*)$/;
const NOT_REGISTERED = /Node type '.+' is not registered/;

export function friendlyError(code: string | undefined, message: string): FriendlyError | null {
  if (code) {
    const known = CODE_MAP[code];
    if (known) return known;
    const m = MISSING_PREFIX.exec(code);
    if (m) {
      const field = m[1].toLowerCase().replace(/_/g, " ");
      return {
        explanation: `A required setting is blank: ${field}.`,
        fix: `Open the node and fill in ${field}, or connect a node that supplies it.`,
      };
    }
  }
  if (NOT_REGISTERED.test(message)) {
    return {
      explanation: "This node's type is not available in this copy of Aerini.",
      fix: "Install the plugin in the Plugins tab, or replace the node.",
    };
  }
  return null;
}

export function friendlyHint(code: string | undefined, message: string): string {
  const f = friendlyError(code, message);
  return f ? `${f.explanation} ${f.fix}` : "";
}

/** The engine logs a failed node twice: once with the node id and once with the error code but no node id. This joins them. */
export function codeForLog(
  log: { node_id?: string; code?: string },
  logs: ReadonlyArray<{ message: string; code?: string }>,
): string | undefined {
  if (log.code) return log.code;
  if (!log.node_id) return undefined;
  const prefix = `Node '${log.node_id}' failed:`;
  return logs.find((l) => l.code && l.message.startsWith(prefix))?.code;
}
