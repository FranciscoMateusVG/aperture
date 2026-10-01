import Foundation
import Darwin

struct LauncherConfig: Decodable {
    let server: String
    let node: String
}
final class NativeControl: ServerControl {
    let config: LauncherConfig
    let home = FileManager.default.homeDirectoryForCurrentUser
    private var server: Process?
    private var serverLog: FileHandle?
    init(config: LauncherConfig) throws {
        for path in [config.server, config.node] {
            guard path.hasPrefix("/"), FileManager.default.isExecutableFile(atPath: path) else {
                throw LauncherError("Pacote local indisponível. Reinstale o lançador; nenhum servidor foi iniciado.")
            }
        }
        self.config = config
    }
    private func process(_ arguments: [String]) -> Process {
        let p = Process()
        p.executableURL = URL(fileURLWithPath: config.server)
        p.arguments = arguments
        p.currentDirectoryURL = home
        p.environment = ["HOME": home.path, "USER": NSUserName(), "LOGNAME": NSUserName(),
                         "PATH": "/opt/homebrew/bin:/usr/bin:/bin:/usr/sbin:/sbin",
                         "TMPDIR": NSTemporaryDirectory(), "APERTURE_NODE_BIN": config.node]
        p.standardInput = FileHandle.nullDevice
        return p
    }
    private func privateFile(_ url: URL) throws -> FileHandle {
        let fd = Darwin.open(url.path, O_RDWR | O_CREAT | O_EXCL | O_NOFOLLOW | O_CLOEXEC, 0o600)
        guard fd >= 0 else { throw LauncherError("Não foi possível criar o log privado do lançador.") }
        return FileHandle(fileDescriptor: fd, closeOnDealloc: true)
    }
    private func command(_ argument: String, seconds: Double = 8) throws -> String {
        let url = FileManager.default.temporaryDirectory.appendingPathComponent("aperture-launcher-\(UUID().uuidString).log")
        let output = try privateFile(url)
        defer { try? output.close(); try? FileManager.default.removeItem(at: url) }
        let p = process([argument])
        p.standardOutput = output; p.standardError = output
        let done = DispatchSemaphore(value: 0)
        p.terminationHandler = { _ in done.signal() }
        try p.run()
        if done.wait(timeout: .now() + seconds) == .timedOut {
            // Only this unreused, directly owned CLI helper. Never the server,
            // hub, agent, process group, or a PID looked up by name/port.
            p.terminate()
            _ = done.wait(timeout: .now() + 2)
            throw LauncherError("A operação demorou demais. Estado não confirmado; não houve nova tentativa nem parada forçada do servidor.")
        }
        try output.seek(toOffset: 0)
        let data = try output.read(upToCount: 8193) ?? Data()
        guard data.count <= 8192 else { throw LauncherError("Resposta do servidor excedeu o limite.") }
        let text = String(decoding: data, as: UTF8.self).trimmingCharacters(in: .whitespacesAndNewlines)
        guard p.terminationStatus == 0 else {
            throw LauncherError(text.isEmpty ? "O servidor recusou a operação." : String(text.prefix(2048)))
        }
        return text
    }
    func status() throws -> ServerState {
        guard let state = ServerState(rawValue: try command("status")) else {
            throw LauncherError("O servidor respondeu com estado desconhecido. Nenhuma inicialização foi tentada.")
        }
        return state
    }
    func start() throws {
        guard server?.isRunning != true else { throw LauncherError("Este lançador já está iniciando o servidor.") }
        let logs = home.appendingPathComponent("Library/Logs/Aperture")
        if !FileManager.default.fileExists(atPath: logs.path) {
            try FileManager.default.createDirectory(at: logs, withIntermediateDirectories: true,
                                                     attributes: [.posixPermissions: 0o700])
        }
        var info = stat()
        guard lstat(logs.path, &info) == 0, (info.st_mode & S_IFMT) == S_IFDIR,
              info.st_uid == geteuid(), info.st_mode & 0o077 == 0 else {
            throw LauncherError("O diretório de logs não é privado; nada foi iniciado.")
        }
        let log = try privateFile(logs.appendingPathComponent("server-\(UUID().uuidString).log"))
        let p = process([])
        p.standardOutput = log; p.standardError = log
        try p.run()
        serverLog = log; server = p
    }
    func waitUntilReady() throws {
        let deadline = Date().addingTimeInterval(30)
        repeat {
            if server?.isRunning != true {
                throw LauncherError("O servidor não iniciou. Consulte o log mais recente em ~/Library/Logs/Aperture. Nenhuma nova tentativa foi feita.")
            }
            if (try? status()) == .running { return }
            Thread.sleep(forTimeInterval: 0.25)
        } while Date() < deadline
        throw LauncherError("A inicialização não foi confirmada. Consulte ~/Library/Logs/Aperture; nenhum servidor foi morto ou iniciado novamente.")
    }
    func openPanel() throws { _ = try command("open", seconds: 10) }
    func stop() throws { _ = try command("stop", seconds: 35) }
}
