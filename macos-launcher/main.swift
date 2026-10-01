import AppKit

final class LauncherDelegate: NSObject, NSApplicationDelegate {
    private var core: LauncherCore?
    private var window: NSWindow!
    private var label: NSTextField!
    private var buttons: [NSButton] = []
    private var statusItem: NSStatusItem!
    private var busy = false
    private var quitting = false

    func applicationDidFinishLaunching(_ notification: Notification) {
        buildUI()
        do {
            guard let url = Bundle.main.url(forResource: "Launcher", withExtension: "json") else {
                throw LauncherError("Configuração do lançador ausente. Reinstale o aplicativo.")
            }
            let config = try JSONDecoder().decode(LauncherConfig.self, from: Data(contentsOf: url))
            core = LauncherCore(try NativeControl(config: config))
            openPanel()
        } catch { showError(error) }
    }
    private func item(_ title: String, _ action: Selector, key: String = "") -> NSMenuItem {
        let i = NSMenuItem(title: title, action: action, keyEquivalent: key); i.target = self; return i
    }
    private func menu() -> NSMenu {
        let m = NSMenu()
        m.addItem(item("Abrir painel", #selector(openPanel)))
        m.addItem(item("Mostrar controles", #selector(showControls)))
        m.addItem(.separator())
        m.addItem(item("Encerrar servidor…", #selector(stopServer)))
        m.addItem(.separator())
        m.addItem(item("Sair do lançador…", #selector(quitLauncher), key: "q"))
        return m
    }
    private func buildUI() {
        let main = NSMenu(), app = NSMenuItem(); app.submenu = menu(); main.addItem(app)
        NSApp.mainMenu = main
        statusItem = NSStatusBar.system.statusItem(withLength: NSStatusItem.squareLength)
        let image = NSApp.applicationIconImage.copy() as! NSImage
        image.size = NSSize(width: 20, height: 20)
        statusItem.button?.image = image; statusItem.button?.toolTip = "Aperture Web"
        statusItem.menu = menu()
        window = NSWindow(contentRect: NSRect(x: 0, y: 0, width: 430, height: 232),
                          styleMask: [.titled, .closable, .miniaturizable], backing: .buffered, defer: false)
        window.title = "Aperture Web"; window.isReleasedWhenClosed = false; window.center()
        let stack = NSStackView(); stack.orientation = .vertical; stack.alignment = .leading; stack.spacing = 14
        stack.translatesAutoresizingMaskIntoConstraints = false
        let title = NSTextField(labelWithString: "Aperture, sem a instalação toda vez.")
        title.font = .boldSystemFont(ofSize: 17); stack.addArrangedSubview(title)
        label = NSTextField(wrappingLabelWithString: "Verificando servidor…")
        stack.addArrangedSubview(label)
        let hint = NSTextField(wrappingLabelWithString: "Fechar o navegador não desliga o servidor. Encerrar o servidor mantém os agentes e o hub ligados.")
        hint.textColor = .secondaryLabelColor; stack.addArrangedSubview(hint)
        let row = NSStackView(); row.spacing = 12
        for (title, action) in [("Abrir painel", #selector(openPanel)), ("Encerrar servidor…", #selector(stopServer))] {
            let b = NSButton(title: title, target: self, action: action); b.bezelStyle = .rounded
            buttons.append(b); row.addArrangedSubview(b)
        }
        stack.addArrangedSubview(row)
        window.contentView!.addSubview(stack)
        NSLayoutConstraint.activate([stack.leadingAnchor.constraint(equalTo: window.contentView!.leadingAnchor, constant: 24),
            stack.trailingAnchor.constraint(equalTo: window.contentView!.trailingAnchor, constant: -24),
            stack.topAnchor.constraint(equalTo: window.contentView!.topAnchor, constant: 24)])
        showControls()
    }
    private func operation(_ text: String, _ action: @escaping (LauncherCore) throws -> Void, success: String) {
        guard !busy, let core = core else { return }
        busy = true; label.stringValue = text; buttons.forEach { $0.isEnabled = false }
        DispatchQueue.global(qos: .userInitiated).async {
            var error: Error?
            do { try action(core) } catch let e { error = e }
            DispatchQueue.main.async {
                self.busy = false; self.buttons.forEach { $0.isEnabled = true }
                if let error = error { self.showError(error) }
                else { self.label.stringValue = success }
            }
        }
    }
    private func showError(_ error: Error) {
        label.stringValue = "Não foi possível concluir."
        showControls()
        let a = NSAlert(); a.messageText = "Aperture Web"; a.informativeText = error.localizedDescription
        a.alertStyle = .warning; a.addButton(withTitle: "OK"); a.runModal()
    }
    @objc func showControls() { window.makeKeyAndOrderFront(nil); NSApp.activate(ignoringOtherApps: true) }
    @objc func openPanel() {
        operation("Abrindo painel…", { try $0.open() }, success: "Servidor ligado. Painel aberto no navegador.")
    }
    @objc func stopServer() {
        guard !busy else { return }
        showControls()
        let a = NSAlert(); a.messageText = "Encerrar o servidor do painel?"
        a.informativeText = "O painel ficará indisponível. Os agentes e o hub continuam rodando. Para ligar o painel novamente, clique em Abrir painel."
        a.addButton(withTitle: "Encerrar servidor"); a.addButton(withTitle: "Cancelar")
        guard a.runModal() == .alertFirstButtonReturn else { return }
        operation("Encerrando o servidor…", { try $0.stop() }, success: "Servidor desligado. Agentes e hub não foram encerrados.")
    }
    @objc func quitLauncher() { NSApp.terminate(nil) }
    func applicationShouldTerminate(_ sender: NSApplication) -> NSApplication.TerminateReply {
        if quitting { return .terminateNow }
        if busy { NSSound.beep(); return .terminateCancel }
        let a = NSAlert(); a.messageText = "Sair do lançador?"
        a.informativeText = "Sair mantém o servidor e os agentes rodando. Para desligar o painel primeiro, use Encerrar servidor."
        a.addButton(withTitle: "Sair sem parar o servidor"); a.addButton(withTitle: "Cancelar")
        if a.runModal() == .alertFirstButtonReturn { quitting = true; return .terminateNow }
        return .terminateCancel
    }
    func applicationShouldHandleReopen(_ sender: NSApplication, hasVisibleWindows: Bool) -> Bool {
        showControls(); openPanel(); return true
    }
    func applicationShouldTerminateAfterLastWindowClosed(_ sender: NSApplication) -> Bool { false }
}
let app = NSApplication.shared
let delegate = LauncherDelegate()
app.setActivationPolicy(.regular)
app.delegate = delegate
app.run()
