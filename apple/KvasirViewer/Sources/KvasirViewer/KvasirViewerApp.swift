import SwiftUI

@main
struct KvasirViewerApp: App {
    @StateObject private var model = ProductionModelFactory.make()

    var body: some Scene {
        WindowGroup("kvasir") {
            OverviewScreen(model: model)
                .task {
                    do {
                        try await model.start()
                    } catch {
                        model.record(error: error)
                    }
                }
        }
    }
}
