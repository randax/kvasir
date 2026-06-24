import Foundation
import KvasirViewerCore

#if canImport(kvasir_client)
import kvasir_client
#endif

enum ProductionModelFactory {
    @MainActor
    static func make(
        overviewClient: (any OverviewClient)? = nil,
        daemonStarter: any DaemonProcessStarter = BundledDaemonProcess.shared,
        daemonFallbackGate: DaemonFallbackGate = DaemonFallbackGate(),
        launchAgent: DaemonLaunchAgent = DaemonLaunchAgent(),
        shouldStartBundledDaemonAfterOverviewError: @escaping @Sendable (any Error) -> Bool =
            ProductionModelFactory.shouldStartBundledDaemonAfterOverviewError
    ) -> KvasirViewerModel {
        KvasirViewerModel(
            dashboard: OverviewDashboard(
                client: makeOverviewClient(
                    primary: overviewClient,
                    starter: daemonStarter,
                    shouldStartDaemonAfterError: { error in
                        daemonFallbackGate.isEnabled && shouldStartBundledDaemonAfterOverviewError(error)
                    }
                )
            ),
            telemetrySetup: makeHarnessTelemetrySetup(),
            launchAgent: launchAgent,
            shouldRefreshLaunchAgentAfterStartupOverviewError: shouldRefreshLaunchAgentAfterStartupOverviewError,
            enablePostStartupOverviewRecovery: {
                daemonFallbackGate.enable()
            }
        )
    }

    @MainActor
    private static func makeOverviewClient(
        primary: (any OverviewClient)?,
        starter: any DaemonProcessStarter,
        shouldStartDaemonAfterError: @escaping @Sendable (any Error) -> Bool
    ) -> any OverviewClient {
        if let primary {
            return DaemonFallbackOverviewClient(
                primary: primary,
                starter: starter,
                shouldStartDaemonAfterError: shouldStartDaemonAfterError
            )
        }
        #if canImport(kvasir_client)
        let socketClient = OverviewSocketClient(
            source: KvasirClientRollupSource(
                socketPath: rpcSocketPath
            )
        )
        return DaemonFallbackOverviewClient(
            primary: socketClient,
            starter: starter,
            shouldStartDaemonAfterError: shouldStartDaemonAfterError
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
        resolvedHarnessTelemetrySetupConfig(environment: ProcessInfo.processInfo.environment)
    }

    static func resolvedHarnessTelemetrySetupConfig(
        environment: [String: String]
    ) -> HarnessTelemetrySetupConfig {
        let home = FileManager.default.homeDirectoryForCurrentUser
        return HarnessTelemetrySetupConfig(
            codexConfigPath: home
                .appendingPathComponent(".codex", isDirectory: true)
                .appendingPathComponent("config.toml")
                .path,
            claudeSettingsPath: claudeSettingsPath(environment: environment, home: home),
            copilotProfilePath: home.appendingPathComponent(".profile").path,
            opencodeConfigPath: home
                .appendingPathComponent(".config", isDirectory: true)
                .appendingPathComponent("opencode", isDirectory: true)
                .appendingPathComponent("opencode.json")
                .path,
            opencodeEnvPath: home
                .appendingPathComponent(".config", isDirectory: true)
                .appendingPathComponent("opencode", isDirectory: true)
                .appendingPathComponent("kvasir.env")
                .path,
            zshProfilePath: home.appendingPathComponent(".zshrc").path,
            bashProfilePath: home.appendingPathComponent(".bashrc").path,
            zshRepoHookPath: home
                .appendingPathComponent(".kvasir", isDirectory: true)
                .appendingPathComponent("repo-hook.zsh")
                .path,
            bashRepoHookPath: home
                .appendingPathComponent(".kvasir", isDirectory: true)
                .appendingPathComponent("repo-hook.bash")
                .path,
            rawBodyDirectory: rawBodyDirectory(environment: environment).path,
            otlpEndpoint: otlpEndpoint(environment: environment)
        )
    }

    private static func claudeSettingsPath(environment: [String: String], home: URL) -> String {
        if let settingsPath = nonEmptyEnvironmentValue("KVASIR_SETUP_SETTINGS", in: environment) {
            return URL(fileURLWithPath: settingsPath).path
        }
        return home
            .appendingPathComponent(".claude", isDirectory: true)
            .appendingPathComponent("settings.json")
            .path
    }

    private static func rawBodyDirectory(environment: [String: String]) -> URL {
        if let dataDirectory = nonEmptyEnvironmentValue("KVASIR_DATA_DIR", in: environment) {
            return URL(fileURLWithPath: dataDirectory, isDirectory: true)
                .appendingPathComponent("raw-bodies", isDirectory: true)
        }
        return applicationSupportDirectory
            .appendingPathComponent("dev.kvasir", isDirectory: true)
            .appendingPathComponent("raw-bodies", isDirectory: true)
    }

    private static func otlpEndpoint(environment: [String: String]) -> String {
        if let bind = nonEmptyEnvironmentValue("KVASIR_OTLP_BIND", in: environment) {
            return "http://\(bind)"
        }
        return "http://127.0.0.1:4318"
    }

    private static func nonEmptyEnvironmentValue(
        _ name: String,
        in environment: [String: String]
    ) -> String? {
        guard let value = environment[name], !value.isEmpty else {
            return nil
        }
        return value
    }

    private static func shouldRefreshLaunchAgentAfterStartupOverviewError(_ error: any Error) -> Bool {
        isRecoverableOverviewTransportFailure(error)
    }

    private static var shouldStartBundledDaemonAfterOverviewError: @Sendable (any Error) -> Bool {
        { error in isRecoverableOverviewTransportFailure(error) }
    }

    private static func isRecoverableOverviewTransportFailure(_ error: any Error) -> Bool {
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
    let copilotProfilePath: String
    let opencodeConfigPath: String
    let opencodeEnvPath: String
    let zshProfilePath: String
    let bashProfilePath: String
    let zshRepoHookPath: String
    let bashRepoHookPath: String
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
                    copilotProfilePath: config.copilotProfilePath,
                    opencodeConfigPath: config.opencodeConfigPath,
                    opencodeEnvPath: config.opencodeEnvPath,
                    zshProfilePath: config.zshProfilePath,
                    bashProfilePath: config.bashProfilePath,
                    zshRepoHookPath: config.zshRepoHookPath,
                    bashRepoHookPath: config.bashRepoHookPath,
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
