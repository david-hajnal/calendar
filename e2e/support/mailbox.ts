import { readFile } from "node:fs/promises";

export interface MailboxMessage {
  recipient: string;
  message_type: "invitation" | "login_link" | "notification";
  authentication_link?: string;
}

/** Reads the append-only E2E outbox; callers wait on a UI/API condition, never a timer. */
export async function messages(outbox: string): Promise<MailboxMessage[]> {
  try {
    return (await readFile(outbox, "utf8"))
      .trim()
      .split("\n")
      .filter(Boolean)
      .map((line) => JSON.parse(line) as MailboxMessage);
  } catch (error: unknown) {
    if ((error as NodeJS.ErrnoException).code === "ENOENT") return [];
    throw error;
  }
}
