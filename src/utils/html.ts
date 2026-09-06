// Single escapeHtml for every innerHTML template in the launcher. Anything
// that is not a closed, trusted set (bead titles from BEADS, version strings
// from the binary, agent name/model echoed into the restart prompt) goes
// through here before landing in markup.
export function escapeHtml(s: string): string {
  return s
    .replace(/&/g, "&amp;")
    .replace(/</g, "&lt;")
    .replace(/>/g, "&gt;")
    .replace(/"/g, "&quot;")
    .replace(/'/g, "&#39;");
}
