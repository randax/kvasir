import Testing

@testable import KvasirViewer

@MainActor
@Test
func productionViewerTargetBuildsOverviewScreenAndFactoryModel() async throws {
    let model = ProductionModelFactory.make()
    _ = OverviewScreen(model: model)

    #if !canImport(kvasir_client)
    do {
        try await model.refreshOverview()
        Issue.record("expected missing kvasir-client error from package-test build")
    } catch {
        #expect(error.localizedDescription.contains("kvasir-client"))
    }
    #endif
}
