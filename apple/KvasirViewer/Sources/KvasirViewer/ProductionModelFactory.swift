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
            launchAgent: DaemonLaunchAgent()
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
}

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
