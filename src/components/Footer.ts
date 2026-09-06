import { commands } from "../services/tauri-commands";
import { escapeHtml } from "../utils/html";

// Bottom-of-launcher version line. Shows semver + short git SHA + UTC build
// date so the operator can verify a reinstall actually picked up the latest
// commit. The values are baked into the Rust binary at build time
// (src-tauri/build.rs writes APERTURE_GIT_SHA and APERTURE_BUILD_DATE env
// vars; the get_version Tauri command reads them via env!()), which is what
// makes this useful as a reinstall verification surface — the SHA shown
// always matches the SHA the binary was built from, not the current HEAD.

export async function createFooter(container: HTMLElement): Promise<void> {
  try {
    const v = await commands.getVersion();
    container.innerHTML = `
      <div class="sidebar-footer__row sidebar-footer__row--version">
        <span class="sidebar-footer__pill">v${escapeHtml(v.semver)}</span>
      </div>
      <div class="sidebar-footer__row sidebar-footer__row--meta">
        <span class="sidebar-footer__sha">${escapeHtml(v.sha)}</span>
        <span class="sidebar-footer__sep">·</span>
        <span class="sidebar-footer__date">${escapeHtml(v.built_at)}</span>
      </div>
    `;
  } catch (e) {
    // If get_version fails (e.g. running in dev with a stale binary), don't
    // crash the launcher — just leave the footer empty.
    console.warn("Footer: failed to load version metadata", e);
  }
}
