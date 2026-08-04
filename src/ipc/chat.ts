import { invoke } from "@tauri-apps/api/core";

export interface ChatImageFileWire {
  filename:  string;
  data:      string;
  mime_type: string;
}

export interface ChatMessageWire {
  id:        string;
  role:      string;
  text?:     string | null;
  images?:   ChatImageFileWire[] | null;
  timestamp: number;
}

export interface ChatSessionWire {
  id:          string;
  workflow_id: string;
  name:        string;
  created_at:  number;
  messages:    ChatMessageWire[];
}

export async function listChatSessions(workflowId: string): Promise<ChatSessionWire[]> {
  return invoke("list_chat_sessions", { workflowId });
}

export async function saveChatSession(session: ChatSessionWire): Promise<void> {
  return invoke("save_chat_session", { session });
}

export async function deleteChatSession(id: string): Promise<void> {
  return invoke("delete_chat_session", { id });
}
