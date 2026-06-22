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

export async function getCredentialMetadata(id: string): Promise<CredentialMetadata | null> {
  return invoke("get_credential_metadata", { id });
}

export async function saveCredential(req: CreateCredentialRequest): Promise<void> {
  return invoke("save_credential", { req });
}

export async function deleteCredential(id: string): Promise<void> {
  return invoke("delete_credential", { id });
}
