import Foundation
import ServiceManagement
import KvasirViewerCore

protocol DaemonProcessStarter: Sendable {
    func startDaemon() throws
}

struct DaemonFallbackOverviewClient: OverviewClient {
    let primary: any OverviewClient
    let starter: any DaemonProcessStarter
    let shouldStartDaemonAfterError: @Sendable (any Error) -> Bool
    let maximumRetryCount: Int
    let retryDelay: @Sendable (Int) async -> Void

    init(
        primary: any OverviewClient,
        starter: any DaemonProcessStarter,
        shouldStartDaemonAfterError: @escaping @Sendable (any Error) -> Bool,
        maximumRetryCount: Int = 20,
        retryDelay: @escaping @Sendable (Int) async -> Void = { attempt in
            let milliseconds = min(50 * attempt, 250)
            try? await Task.sleep(nanoseconds: UInt64(milliseconds) * 1_000_000)
        }
    ) {
        self.primary = primary
        self.starter = starter
        self.shouldStartDaemonAfterError = shouldStartDaemonAfterError
        self.maximumRetryCount = maximumRetryCount
        self.retryDelay = retryDelay
    }

    func loadOverviewSnapshot(query: OverviewQuery) async throws -> OverviewSnapshot {
        do {
            return try await primary.loadOverviewSnapshot(query: query)
        } catch {
            guard shouldStartDaemonAfterError(error) else {
                throw error
            }
            try starter.startDaemon()
            return try await loadAfterDaemonStart(query: query)
        }
    }

    private func loadAfterDaemonStart(query: OverviewQuery) async throws -> OverviewSnapshot {
        var retry = 0
        while true {
            do {
                return try await primary.loadOverviewSnapshot(query: query)
            } catch {
                guard retry < maximumRetryCount, shouldStartDaemonAfterError(error) else {
                    throw error
                }
                retry += 1
                await retryDelay(retry)
            }
        }
    }

}

final class DaemonFallbackGate: @unchecked Sendable {
    private let lock = NSLock()
    private var enabled: Bool

    init(enabled: Bool = false) {
        self.enabled = enabled
    }

    var isEnabled: Bool {
        lock.lock()
        defer { lock.unlock() }
        return enabled
    }

    func enable() {
        lock.lock()
        defer { lock.unlock() }
        enabled = true
    }
}

final class BundledDaemonProcess: DaemonProcessStarter, @unchecked Sendable {
    static let shared = BundledDaemonProcess()

    private let lock = NSLock()
    private var process: Process?

    private init() {}

    func startDaemon() throws {
        lock.lock()
        defer { lock.unlock() }

        try? SMAppService.agent(plistName: DaemonLaunchAgent.plistName).unregister()
        if let process, process.isRunning {
            process.terminate()
            process.waitUntilExit()
        }

        let daemonURL = try Self.daemonURL()
        let process = Process()
        process.executableURL = daemonURL
        process.environment = daemonEnvironment()
        try process.run()
        self.process = process
    }

    private static func daemonURL() throws -> URL {
        guard let executableDirectory = Bundle.main.executableURL?.deletingLastPathComponent() else {
            throw BundledDaemonProcessError.missingBundleExecutable
        }
        let url = executableDirectory.appendingPathComponent("kvasird")
        guard FileManager.default.isExecutableFile(atPath: url.path) else {
            throw BundledDaemonProcessError.missingDaemon(url.path)
        }
        return url
    }

    private func daemonEnvironment() -> [String: String] {
        Self.daemonEnvironment(
            processEnvironment: ProcessInfo.processInfo.environment,
            homeDirectory: FileManager.default.homeDirectoryForCurrentUser
        )
    }

    static func daemonEnvironment(
        processEnvironment: [String: String],
        homeDirectory: URL
    ) -> [String: String] {
        var environment = processEnvironment
        if environment["HOME", default: ""].isEmpty {
            environment["HOME"] = homeDirectory.path
        }
        return environment
    }
}

enum BundledDaemonProcessError: LocalizedError {
    case missingBundleExecutable
    case missingDaemon(String)

    var errorDescription: String? {
        switch self {
        case .missingBundleExecutable:
            return "Kvasir.app executable path is unavailable"
        case .missingDaemon(let path):
            return "Bundled kvasird is not executable at \(path)"
        }
    }
}
