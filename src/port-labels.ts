import { getNodeDescriptor } from "./canvas/node-registry";

const OUTPUT_LABEL_UPGRADES: Readonly<Record<string, string>> = { Out: "Result" };

export function displayPortLabel(typeId: string, portId: string, label: string): string {
  if (portId !== "output") return label;
  const desc = getNodeDescriptor(typeId);
  if (!desc || desc.is_plugin) return label;
  return OUTPUT_LABEL_UPGRADES[label] ?? label;
}
