import { NODE_IDS } from "./node-ids";

/** Config keys whose value changes which fields or ports a node shows. */
const STRUCTURAL_KEYS: ReadonlySet<string> = new Set(["provider", "db_type", "mode", "operation", "cron_expr"]);

/** Node types whose ports or layout depend on their config, so a value that is only known at run time would break them. */
const STRUCTURAL_NODE_TYPES: ReadonlySet<string> = new Set([
  NODE_IDS.COLLECT_FILES, NODE_IDS.SAVE_TO_FOLDER, NODE_IDS.SWITCH, NODE_IDS.LOOP, NODE_IDS.SCHEDULE, "if",
]);

export interface DataSwitchProp {
  type?: string;
  enum?: string[];
}

/** True for a value that holds a reference: a string with `{{`. */
export function isDataValue(value: unknown): boolean {
  return typeof value === "string" && value.includes("{{");
}

/** True when the number, true-or-false or dropdown field may be switched to data.
 *  The engine converts the result back to a number or boolean (`coerce_scalar_expressions`),
 *  except for safety switches (`allow_*`), which stay fixed. Integer fields are not listed:
 *  they already show a text box that takes references. */
export function dataSwitchEligible(nodeTypeId: string, key: string, prop: DataSwitchProp): boolean {
  if (key.startsWith("allow_")) return false;
  if (STRUCTURAL_KEYS.has(key) || STRUCTURAL_NODE_TYPES.has(nodeTypeId)) return false;
  if (prop.enum) return true;
  return prop.type === "number" || prop.type === "boolean";
}
