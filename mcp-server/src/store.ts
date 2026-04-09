import { mkdirSync, writeFileSync, readdirSync, readFileSync, unlinkSync } from "node:fs";
import { join, resolve } from "node:path";
import { homedir } from "node:os";

export class MailboxStore {
  private baseDir: string;

  constructor(baseDir?: string) {
    this.baseDir = baseDir ?? resolve(homedir(), ".aperture", "mailbox");
    mkdirSync(this.baseDir, { recursive: true });
  }

  ensureMailbox(agentName: string): string {
    const dir = join(this.baseDir, agentName);
    mkdirSync(dir, { recursive: true });
    return dir;
  }

  sendMessage(from: string, to: string, content: string): string {
    const mailboxDir = this.ensureMailbox(to);
    const timestamp = Date.now();
    const filename = `${timestamp}-${from}.md`;
    const filepath = join(mailboxDir, filename);
    const fileContent = `# Message from ${from}\n_${new Date().toISOString()}_\n\n${content}\n`;
    writeFileSync(filepath, fileContent, "utf-8");
    return filepath;
  }

  listPendingMessages(agentName: string): string[] {
    const dir = this.ensureMailbox(agentName);
    try {
      return readdirSync(dir)
        .filter(f => f.endsWith(".md"))
        .sort()
        .map(f => join(dir, f));
    } catch {
      return [];
    }
  }

  readAndDelete(filepath: string): string {
    const content = readFileSync(filepath, "utf-8");
    unlinkSync(filepath);
    return content;
  }
}
