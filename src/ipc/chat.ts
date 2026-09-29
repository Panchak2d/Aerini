import { invoke } from "@tauri-apps/api/core";

export interface ChatImageFileWire {
  filename:  string;
  data:      string;
  mime_type: string;
}

export interface ChatAttachmentWire {
  filename:  string;
  data:      string;
  mime_type: string;
}

export interface ChatMessageWire {
  id:           string;
  role:         string;
  text?:        string | null;
  images?:      ChatImageFileWire[] | null;
  attachments?: ChatAttachmentWire[] | null;
  timestamp:    number;
}

export interface ChatSessionWire {
  id:          string;
  workflow_id: string;
  name:        string;
  created_at:  number;
  messages:    ChatMessageWire[];
}

export interface ChatSessionMetaWire {
  id:          string;
  workflow_id: string;
  name:        string;
  created_at:  number;
}

/** A workflow's sessions without their messages, newest-created first. */
export async function listChatSessionMeta(workflowId: string): Promise<ChatSessionMetaWire[]> {
  return invoke("list_chat_session_meta", { workflowId });
}

/** One session's messages in chronological order. */
export async function loadChatMessages(sessionId: string): Promise<ChatMessageWire[]> {
  return invoke("load_chat_messages", { sessionId });
}

/** Adds one message and upserts its session row. Repeating a call with the same message id does not add a second row. */
export async function appendChatMessage(session: ChatSessionMetaWire, message: ChatMessageWire): Promise<void> {
  return invoke("append_chat_message", { session, message });
}

/** Upserts a session's metadata without touching its messages. */
export async function saveChatSessionMeta(session: ChatSessionMetaWire): Promise<void> {
  return invoke("save_chat_session_meta", { session });
}

/** Replaces a session's whole message list. Sends every message, so it is only for one-off bulk imports. */
export async function saveChatSession(session: ChatSessionWire): Promise<void> {
  return invoke("save_chat_session", { session });
}

export async function deleteChatSession(id: string): Promise<void> {
  return invoke("delete_chat_session", { id });
}
