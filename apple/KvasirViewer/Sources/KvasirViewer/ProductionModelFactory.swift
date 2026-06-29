import Foundation
import KvasirViewerCore

#if canImport(kvasir_client)
import kvasir_client
#endif

enum ProductionModelFactory {
    @MainActor
    static func make(
        overviewClient: (any OverviewClient)? = nil,
        traceInspectorClient: (any TraceInspectorClient)? = nil,
        overviewUpdateSource: (any OverviewUpdateSource)? = nil,
        usageDataManagement: (any UsageDataManagement)? = nil,
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
                ),
                updateSource: overviewUpdateSource ?? makeOverviewUpdateSource(primary: overviewClient)
            ),
            traceInspector: makeTraceInspector(client: traceInspectorClient),
            usageDataManagement: usageDataManagement
                ?? makeUsageDataManagement(usesInjectedOverviewClient: overviewClient != nil),
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
                socketPath: rpcSocketPath,
                setupConfig: harnessTelemetrySetupConfig
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

    @MainActor
    private static func makeTraceInspector(
        client primary: (any TraceInspectorClient)?
    ) -> TraceInspector? {
        if let primary {
            return TraceInspector(client: primary)
        }
        #if canImport(kvasir_client)
        let socketClient = TraceInspectorSocketClient(
            source: KvasirClientRollupSource(
                socketPath: rpcSocketPath,
                setupConfig: harnessTelemetrySetupConfig
            )
        )
        return TraceInspector(client: socketClient)
        #else
        return nil
        #endif
    }

    @MainActor
    private static func makeOverviewUpdateSource(primary: (any OverviewClient)?) -> (any OverviewUpdateSource)? {
        guard primary == nil else {
            return nil
        }
        #if canImport(kvasir_client)
        return KvasirClientUsageUpdateSource(socketPath: rpcSocketPath)
        #else
        return nil
        #endif
    }

    @MainActor
    private static func makeUsageDataManagement(usesInjectedOverviewClient: Bool) -> any UsageDataManagement {
        guard !usesInjectedOverviewClient else {
            return UnavailableUsageDataManagement()
        }
        #if canImport(kvasir_client)
        return KvasirClientUsageDataManagement(
            socketPath: rpcSocketPath,
            setupConfig: harnessTelemetrySetupConfig
        )
        #else
        return UnavailableUsageDataManagement()
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
        return clientError == .SocketIo || clientError == .RpcSerialization
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

struct HarnessTelemetrySetupWarning: LocalizedError, Equatable, Sendable {
    enum Reason: Equatable, Sendable {
        case invalidClaudeSettings
        case invalidCodexConfig
        case invalidOpenCodeConfig
        case invalidStoredSecret
        case setupFailed
        case rollbackFailed
        case stateUnknown
        case uninstallConflict
        case filesystem
        case rpcSerialization
        case socketIo
        case rpcResponseTooLarge
        case daemonError
        case wrongResponseType
        case invalidQuery
    }

    let reason: Reason

    var errorDescription: String? {
        switch reason {
        case .invalidClaudeSettings:
            "Claude Code settings are not valid JSON. Fix ~/.claude/settings.json and restart Kvasir."
        case .invalidCodexConfig:
            "Codex telemetry config could not be updated automatically. Check ~/.codex/config.toml for malformed kvasir managed blocks or conflicting [otel] settings."
        case .invalidOpenCodeConfig:
            "OpenCode config is not valid JSON. Fix ~/.config/opencode/opencode.json and restart Kvasir."
        case .invalidStoredSecret:
            "Kvasir's stored telemetry setup secret is invalid. Re-run telemetry setup or remove the stale kvasir setup secret."
        case .setupFailed:
            "Kvasir could not configure Codex telemetry. Check keychain access and your harness config files, then restart Kvasir."
        case .rollbackFailed:
            "Kvasir could not roll back a failed telemetry setup. Check your harness config files before retrying."
        case .stateUnknown:
            "Kvasir could not determine whether telemetry setup completed. Check your harness config files before retrying."
        case .uninstallConflict:
            "Kvasir telemetry uninstall would overwrite local changes."
        case .filesystem:
            "Kvasir could not write telemetry configuration files. Check file permissions for your harness config directories."
        case .rpcSerialization:
            "Kvasir could not serialize telemetry setup data. Check generated kvasir-client bindings and rebuild Kvasir.app."
        case .socketIo:
            "Kvasir could not connect to the daemon socket."
        case .rpcResponseTooLarge:
            "Kvasir daemon returned an unexpectedly large response."
        case .daemonError:
            "Kvasir daemon returned an error."
        case .wrongResponseType:
            "Kvasir daemon returned an unexpected response."
        case .invalidQuery:
            "Kvasir could not build a valid daemon query."
        }
    }
}

struct ConfiguringHarnessTelemetrySetup: HarnessTelemetrySetup {
    let config: HarnessTelemetrySetupConfig
    let configure: @Sendable (HarnessTelemetrySetupConfig) throws -> Void
    let warningForError: @Sendable (any Error) -> HarnessTelemetrySetupWarning?

    func ensureConfigured() async throws {
        let config = config
        let configure = configure
        let warningForError = warningForError
        try await Task.detached(priority: .userInitiated) {
            do {
                try configure(config)
            } catch {
                if let warning = warningForError(error) {
                    throw warning
                }
                throw error
            }
        }.value
    }
}

#if canImport(kvasir_client)
struct KvasirClientHarnessTelemetrySetup: HarnessTelemetrySetup {
    private let setup: ConfiguringHarnessTelemetrySetup

    init(config: HarnessTelemetrySetupConfig) {
        setup = ConfiguringHarnessTelemetrySetup(
            config: config,
            configure: { config in
                try configureKvasirHarnessTelemetry(config: kvasirHarnessTelemetrySetup(from: config))
            },
            warningForError: { error in
                guard let error = error as? KvasirClientError else {
                    return nil
                }
                return HarnessTelemetrySetupWarning(error: error)
            }
        )
    }

    func ensureConfigured() async throws {
        try await setup.ensureConfigured()
    }
}

extension HarnessTelemetrySetupWarning {
    init(error: KvasirClientError) {
        switch error {
        case .HarnessTelemetryInvalidClaudeSettings:
            self.init(reason: .invalidClaudeSettings)
        case .HarnessTelemetryInvalidCodexConfig:
            self.init(reason: .invalidCodexConfig)
        case .HarnessTelemetryInvalidOpenCodeConfig:
            self.init(reason: .invalidOpenCodeConfig)
        case .HarnessTelemetryInvalidStoredSecret:
            self.init(reason: .invalidStoredSecret)
        case .HarnessTelemetrySetup:
            self.init(reason: .setupFailed)
        case .HarnessTelemetryRollback:
            self.init(reason: .rollbackFailed)
        case .HarnessTelemetryStateUnknown:
            self.init(reason: .stateUnknown)
        case .HarnessTelemetryUninstallConflict:
            self.init(reason: .uninstallConflict)
        case .Filesystem:
            self.init(reason: .filesystem)
        case .RpcSerialization:
            self.init(reason: .rpcSerialization)
        case .SocketIo:
            self.init(reason: .socketIo)
        case .RpcResponseTooLarge:
            self.init(reason: .rpcResponseTooLarge)
        case .DaemonError:
            self.init(reason: .daemonError)
        case .WrongResponseType:
            self.init(reason: .wrongResponseType)
        case .InvalidQuery:
            self.init(reason: .invalidQuery)
        }
    }
}
#endif

private struct MissingKvasirClient: OverviewClient {
    func loadOverviewSnapshot(query: OverviewQuery) async throws -> OverviewSnapshot {
        throw MissingKvasirClientError()
    }
}

private struct MissingKvasirClientError: LocalizedError {
    var errorDescription: String? {
        "kvasir-client is not linked; build Kvasir.app with scripts/build-app.sh"
    }
}
