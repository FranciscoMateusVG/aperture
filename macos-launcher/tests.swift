import Foundation

final class Fake: ServerControl {
    var state: ServerState = .running
    var events: [String] = []
    var brokenStatus = false, brokenReady = false, brokenStop = false
    var onStart: (() -> Void)?
    func status() throws -> ServerState { events.append("status"); if brokenStatus { throw LauncherError("wrong service") }; return state }
    func start() throws { events.append("start"); onStart?() }
    func waitUntilReady() throws { events.append("ready"); if brokenReady { throw LauncherError("startup failed") }; state = .running }
    func openPanel() throws { events.append("open") }
    func stop() throws { events.append("stop"); if brokenStop { throw LauncherError("unknown stop") }; state = .stopped }
}
func expectError(_ f: () throws -> Void) { do { try f(); fatalError("expected error") } catch {} }
@main struct Tests {
    static func main() throws {
        let f = Fake()
        let c = LauncherCore(f)
        try c.open(); assert(f.events == ["status", "open"])
        f.events = []; f.state = .stopped
        var duplicateDenied = false
        f.onStart = { do { try c.open() } catch { duplicateDenied = true } }
        try c.open(); assert(duplicateDenied); assert(f.events == ["status", "start", "ready", "open"])
        f.onStart = nil; f.events = []; f.brokenStatus = true
        expectError { try c.open() }; assert(f.events == ["status"])
        f.brokenStatus = false; f.events = []; f.state = .stopped; f.brokenReady = true
        expectError { try c.open() }; assert(f.events == ["status", "start", "ready"])
        f.events = []; f.state = .stopping
        expectError { try c.open() }; assert(f.events == ["status"])
        f.events = []; expectError { try c.stop() }; assert(f.events == ["status"])
        f.events = []; f.state = .running; try c.stop(); assert(f.events == ["status", "stop", "status"])
        f.events = []; try c.stop(); assert(f.events == ["status"])
        f.events = []; f.state = .running; f.brokenStop = true
        expectError { try c.stop() }; assert(f.events == ["status", "stop"])
        print("9 launcher flow cases PASS")
    }
}
