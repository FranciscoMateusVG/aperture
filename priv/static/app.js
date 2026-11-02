// Aperture launcher page: LiveView client. No build step: Phoenix and
// LiveView come as globals from /assets/vendor.
(() => {
  const Hooks = {};

  // Chat composer: Enter sends (Shift+Enter is a new line), pasted images go
  // to the upload (previewed before sending), the field clears once sent.
  // The text is a draft per conversation: switching agents saves it and
  // brings back the other one's (the server never sees unsent text).
  const drafts = {};
  Hooks.ChatInput = {
    mounted() {
      this.agent = this.el.dataset.agent;
      this.el.value = drafts[this.agent] || "";
      this.grow();
      this.el.focus();
      this.el.addEventListener("keydown", (e) => {
        if (e.key === "Enter" && !e.shiftKey && !e.isComposing) {
          e.preventDefault();
          this.el.form.requestSubmit();
        }
      });
      this.el.addEventListener("input", () => {
        drafts[this.agent] = this.el.value;
        this.grow();
      });
      this.el.addEventListener("paste", (e) => {
        const files = [...(e.clipboardData?.files || [])].filter((f) => f.type.startsWith("image/"));
        if (files.length) {
          e.preventDefault();
          this.upload("images", files);
        }
      });
      this.handleEvent("chat:sent", ({ agent }) => {
        delete drafts[agent];
        if (agent !== this.agent) return;
        this.el.value = "";
        this.grow();
        this.el.focus();
      });
    },
    updated() {
      // phx-update="ignore" keeps the text; data-agent still follows the conversation
      if (this.el.dataset.agent === this.agent) return;
      drafts[this.agent] = this.el.value;
      this.agent = this.el.dataset.agent;
      this.el.value = drafts[this.agent] || "";
      this.grow();
      this.el.focus();
    },
    grow() {
      this.el.style.height = "auto";
      this.el.style.height = Math.min(this.el.scrollHeight, 160) + "px";
      this.el.style.overflowY = this.el.scrollHeight > 160 ? "auto" : "hidden";
    },
  };

  // Chat history: start at the bottom; follow new messages unless the
  // operator scrolled up to read; loading older ones keeps the view in place.
  Hooks.ChatScroll = {
    mounted() {
      this.stick = true;
      this.el.addEventListener("scroll", () => {
        this.stick = this.el.scrollHeight - this.el.scrollTop - this.el.clientHeight < 60;
      });
      this.el.addEventListener("click", (e) => {
        if (e.target.closest("#chat-older")) this.fromBottom = this.el.scrollHeight - this.el.scrollTop;
      });
      this.observer = new MutationObserver(() => this.settle());
      this.observer.observe(this.el, { childList: true, subtree: true });
      this.bottom();
      // images load after the patch and push the content down
      this.el.addEventListener("load", () => this.stick && this.bottom(), true);
    },
    destroyed() {
      this.observer.disconnect();
    },
    settle() {
      if (this.fromBottom !== undefined) {
        this.el.scrollTop = this.el.scrollHeight - this.fromBottom;
        this.fromBottom = undefined;
      } else if (this.stick) {
        this.bottom();
      }
    },
    bottom() {
      this.el.scrollTop = this.el.scrollHeight;
    },
  };

  // Installed-app behaviour: registers the service worker, keeps the Dock
  // badge at the unread count, and shows a macOS notification for an agent's
  // message while the window is in the background. Permission is asked only
  // when the operator clicks "Enable notifications".
  Hooks.Notifier = {
    mounted() {
      this.button = this.el.querySelector("button");
      this.button.addEventListener("click", async () => {
        await Notification.requestPermission();
        this.showButton();
      });
      this.showButton();

      if ("serviceWorker" in navigator) {
        navigator.serviceWorker.register("/sw.js").catch((e) => console.warn("sw:", e));
        navigator.serviceWorker.addEventListener("message", (e) => {
          if (e.data?.type === "open-chat") this.pushEvent("chat_open", { agent: e.data.agent });
        });
      }

      this.handleEvent("unread", ({ total }) => {
        if (!("setAppBadge" in navigator)) return;
        (total > 0 ? navigator.setAppBadge(total) : navigator.clearAppBadge()).catch(() => {});
      });

      this.handleEvent("chat:incoming", (m) => this.notify(m));

      // an open conversation only counts as read while the operator can see it
      const report = () => this.reportVisibility();
      document.addEventListener("visibilitychange", report);
      window.addEventListener("focus", report);
      window.addEventListener("blur", report);
      this.reportVisibility(true);
    },
    // a rejoin starts the server side over (not visible): tell it again
    reconnected() {
      this.reportVisibility(true);
    },
    reportVisibility(force = false) {
      const visible = document.visibilityState === "visible" && document.hasFocus();
      if (force || visible !== this.visible) {
        this.visible = visible;
        this.pushEvent("visibility", { visible });
      }
    },
    showButton() {
      this.button.hidden = !("Notification" in window) || Notification.permission !== "default";
    },
    async notify({ id, agent, title, body }) {
      if (!("Notification" in window) || Notification.permission !== "granted") return;
      if (document.visibilityState === "visible" && document.hasFocus()) return;
      const opts = { body, tag: `msg-${id}`, data: { agent }, icon: "/icons/icon-192.png" };
      const reg = "serviceWorker" in navigator && (await navigator.serviceWorker.getRegistration());
      if (reg) return reg.showNotification(title, opts);
      const n = new Notification(title, opts);
      n.onclick = () => { window.focus(); this.pushEvent("chat_open", { agent }); n.close(); };
    },
  };

  const csrf = document.querySelector("meta[name='csrf-token']").getAttribute("content");
  const liveSocket = new LiveView.LiveSocket("/live", Phoenix.Socket, {
    params: { _csrf_token: csrf },
    hooks: Hooks,
  });
  liveSocket.connect();
  window.liveSocket = liveSocket;
})();
