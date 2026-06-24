import Foundation
import KvasirViewerCore

#if canImport(kvasir_client)
import kvasir_client
#endif

enum ProductionModelFactory {
    @MainActor
    static func make() -> KvasirViewerModel {
        KvasirViewerModel(
            dashboard: OverviewDashboard(client: makeOverviewClient()),
            telemetrySetup: makeHarnessTelemetrySetup(),
            launchAgent: DaemonLaunchAgent(),
            shouldRefreshLaunchAgentAfterStartupOverviewError: shouldRefreshLaunchAgentAfterStartupOverviewError
        )
    }

    @MainActor
    private static func makeOverviewClient() -> any OverviewClient {
        #if canImport(kvasir_client)
        return OverviewSocketClient(
            source: KvasirClientRollupSource(
                socketPath: rpcSocketPath
            )
        )
        #else
        return MissingKvasirClient()
        #endif
    }

    private static var rpcSocketPath: String {
        if let override = ProcessInfo.processInfo.environment["KVASIR_RPC_SOCKET"], !override.isEmpty {
            return override
        }
        let applicationSupport = FileManager.default.urls(
            for: .applicationSupportDirectory,
            in: .userDomainMask
        ).first ?? FileManager.default.homeDirectoryForCurrentUser
        return applicationSupport
            .appendingPathComponent("dev.kvasir", isDirectory: true)
            .appendingPathComponent("kvasird.sock")
            .path
    }

    @MainActor
    private static func makeHarnessTelemetrySetup() -> any HarnessTelemetrySetup {
        #if canImport(kvasir_client)
        return KvasirClientHarnessTelemetrySetup(config: harnessTelemetrySetupConfig)
        #else
        return NoOpHarnessTelemetrySetup()
        #endif
    }

    private static var harnessTelemetrySetupConfig: HarnessTelemetrySetupConfig {
        let home = FileManager.default.homeDirectoryForCurrentUser
        return HarnessTelemetrySetupConfig(
            codexConfigPath: home
                .appendingPathComponent(".codex", isDirectory: true)
                .appendingPathComponent("config.toml")
                .path,
            claudeSettingsPath: home
                .appendingPathComponent(".claude", isDirectory: true)
                .appendingPathComponent("settings.json")
                .path,
            rawBodyDirectory: applicationSupportDirectory
                .appendingPathComponent("dev.kvasir", isDirectory: true)
                .appendingPathComponent("raw-bodies", isDirectory: true)
                .path,
            otlpEndpoint: "http://127.0.0.1:4318"
        )
    }

    private static func shouldRefreshLaunchAgentAfterStartupOverviewError(_ error: any Error) -> Bool {
        #if canImport(kvasir_client)
        guard let clientError = error as? KvasirClientError else {
            return false
        }
        return clientError == .SocketIo
        #else
        return false
        #endif
    }

    private static var applicationSupportDirectory: URL {
        FileManager.default.urls(
            for: .applicationSupportDirectory,
            in: .userDomainMask
        ).first ?? FileManager.default.homeDirectoryForCurrentUser
    }
}

struct HarnessTelemetrySetupConfig: Sendable {
    let codexConfigPath: String
    let claudeSettingsPath: String
    let rawBodyDirectory: String
    let otlpEndpoint: String
}

#if canImport(kvasir_client)
struct KvasirClientHarnessTelemetrySetup: HarnessTelemetrySetup {
    let config: HarnessTelemetrySetupConfig

    func ensureConfigured() async throws {
        try await Task.detached(priority: .userInitiated) {
            try configureKvasirHarnessTelemetry(
                config: KvasirHarnessTelemetrySetup(
                    codexConfigPath: config.codexConfigPath,
                    claudeSettingsPath: config.claudeSettingsPath,
                    rawBodyDirectory: config.rawBodyDirectory,
                    otlpEndpoint: config.otlpEndpoint
                )
            )
        }.value
    }
}
#endif

private struct MissingKvasirClient: OverviewClient {
    func loadOverviewRollups(query: OverviewQuery) async throws -> OverviewRollups {
        throw MissingKvasirClientError()
    }
}

private struct MissingKvasirClientError: LocalizedError {
    var errorDescription: String? {
        "kvasir-client is not linked; build Kvasir.app with scripts/build-app.sh"
    }
}
