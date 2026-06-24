import SwiftUI

@main
struct KvasirViewerApp: App {
    @StateObject private var model = ProductionModelFactory.make()
    @Environment(\.scenePhase) private var scenePhase
    @State private var startupCompleted = false

    var body: some Scene {
        WindowGroup("kvasir") {
            let screen = OverviewScreen(model: model)
                .task {
                    await startModel()
                }

            if #available(macOS 14.0, *) {
                screen.onChange(of: scenePhase) { _, phase in
                    refreshWhenReturningToForeground(phase)
                }
            } else {
                screen.onChange(of: scenePhase) { phase in
                    refreshWhenReturningToForeground(phase)
                }
            }
        }
    }

    @MainActor
    private func startModel() async {
        do {
            try await model.start()
        } catch {
            model.record(error: error)
        }
        startupCompleted = true
    }

    @MainActor
    private func refreshWhenReturningToForeground(_ phase: ScenePhase) {
        guard startupCompleted, phase == .active else {
            return
        }
        Task {
            do {
                try await model.refreshOverview()
            } catch {
                model.record(error: error)
            }
        }
    }
}
