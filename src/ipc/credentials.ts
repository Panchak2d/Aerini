import { invoke } from "@tauri-apps/api/core";

export interface CredentialEntry {
  id:        string;
  name:      string;
  cred_type: string;
}

export interface CredentialMetadata {
  provider?: string;
  model?:    string;
  base_url?: string;
}

export interface CreateCredentialRequest {
  id:        string;
  name:      string;
  value:     string;
  cred_type: string;
  provider?: string;
  model?:    string;
  base_url?: string;
}

export async function listCredentials(): Promise<CredentialEntry[]> {
  return invoke("list_credentials");
}

/// Maps credential ID -> distinct workflow names referencing it, for every
/// credential used by at least one workflow. Powers the "used by" display in
/// CredentialPanel's list — computed up front, not just at delete time.
export async function listCredentialUsage(): Promise<Record<string, string[]>> {
  return invoke("list_credential_usage");
}

export async function getCredentialMetadata(id: string): Promise<CredentialMetadata | null> {
  return invoke("get_credential_metadata", { id });
}

/// Creates a credential. With `update` it replaces the existing one instead;
/// an empty `value` then keeps the stored secret. Secrets are write-only from
/// the webview: there is no command that returns a saved value.
export async function saveCredential(req: CreateCredentialRequest, update = false): Promise<void> {
  return invoke("save_credential", { req, update });
}

/// Lists a provider's models using a saved credential's secret, read by the
/// backend and never returned here. Rejects as `"{CODE}: {message}"`, same as
/// `listProviderModels`.
export async function listCredentialModels(id: string, provider: string, baseUrl: string): Promise<string[]> {
  return invoke("list_credential_models", { id, provider, baseUrl });
}

export async function deleteCredential(id: string): Promise<void> {
  return invoke("delete_credential", { id });
}

/// Returns the raw AES-256 credential-encryption key, base64-encoded, after a
/// native confirmation dialog. Rejects with `"CANCELLED"` when the user
/// declines. This is the only recovery path if the OS keychain entry holding
/// it is ever lost.
export async function exportEncryptionKey(): Promise<string> {
  return invoke("export_encryption_key");
}
