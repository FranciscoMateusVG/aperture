export function createStatusBar(container: HTMLElement) {
  container.innerHTML = `
    <div class="status-bar">
      <span class="status-bar__title">APERTURE</span>
      <span class="status-bar__dot status-bar__dot--connected"></span>
    </div>
  `;

  return {
    setConnected(connected: boolean) {
      const dot = container.querySelector('.status-bar__dot')!;
      dot.className = `status-bar__dot status-bar__dot--${connected ? 'connected' : 'disconnected'}`;
    }
  };
}
