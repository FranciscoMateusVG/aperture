export function createNavbar(
  titleEl: HTMLElement,
  actionsEl: HTMLElement,
  onTogglePanel: (panel: string) => void
) {
  titleEl.innerHTML = `
    <span class="navbar__logo">APERTURE</span>
    <span class="navbar__dot navbar__dot--connected"></span>
  `;

  actionsEl.querySelectorAll(".navbar__btn").forEach((btn) => {
    btn.addEventListener("click", () => {
      const panel = (btn as HTMLElement).dataset.panel!;
      onTogglePanel(panel);
    });
  });

  return {
    setConnected(connected: boolean) {
      const dot = titleEl.querySelector(".navbar__dot")!;
      dot.className = `navbar__dot navbar__dot--${connected ? "connected" : "disconnected"}`;
    },
  };
}
