import Foundation

enum ServerState: String { case running, stopped, stopping }
struct LauncherError: LocalizedError {
    let message: String
    init(_ message: String) { self.message = message }
    var errorDescription: String? { message }
}
protocol ServerControl {
    func status() throws -> ServerState
    func start() throws
    func waitUntilReady() throws
    func openPanel() throws
    func stop() throws
}
// The operation lock also covers Dock reopen, menu actions and termination.
// A status error is never permission to start a second server.
final class LauncherCore {
    private let lock = NSLock()
    private let control: ServerControl
    init(_ control: ServerControl) { self.control = control }
    func perform(_ body: () throws -> Void) throws {
        guard lock.try() else { throw LauncherError("Uma operação já está em andamento.") }
        defer { lock.unlock() }
        try body()
    }
    func open() throws {
        try perform {
            switch try control.status() {
            case .running: break
            case .stopped: try control.start(); try control.waitUntilReady()
            case .stopping: throw LauncherError("O servidor está encerrando. Aguarde e abra novamente.")
            }
            try control.openPanel()
        }
    }
    func stop() throws {
        try perform {
            switch try control.status() {
            case .stopped: return
            case .running: try control.stop()
            case .stopping: throw LauncherError("O servidor já está encerrando; nenhum segundo pedido foi enviado.")
            }
            guard try control.status() == .stopped else {
                throw LauncherError("O encerramento não foi confirmado. Nenhuma parada forçada foi feita.")
            }
        }
    }
}
