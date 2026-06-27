import Combine
import Foundation

public enum OverviewRangePreset: String, CaseIterable, Identifiable, Sendable {
    case today
    case lastSevenDays
    case lastThirtyDays

    public var id: String { rawValue }

    public var label: String {
        switch self {
        case .today:
            return "Today"
        case .lastSevenDays:
            return "7 days"
        case .lastThirtyDays:
            return "30 days"
        }
    }

    public func range(containing date: Date, calendar: Calendar) -> OverviewTimeRange {
        let dayStart = calendar.startOfDay(for: date)
        let start: Date
        switch self {
        case .today:
            start = dayStart
        case .lastSevenDays:
            start = calendar.date(byAdding: .day, value: -6, to: dayStart) ?? dayStart
        case .lastThirtyDays:
            start = calendar.date(byAdding: .day, value: -29, to: dayStart) ?? dayStart
        }
        return OverviewTimeRange(start: start, end: date)
    }
}

@MainActor
public final class KvasirViewerModel: ObservableObject {
    @Published public private(set) var overviewSnapshot: OverviewSnapshot?
    @Published public private(set) var traceInspectorSnapshot: TraceInspectorSnapshot?
    @Published public private(set) var launchAgentOutcome: LaunchAgentRegistrationOutcome?
    @Published public private(set) var errorMessage: String?
    @Published public private(set) var traceInspectorErrorMessage: String?
    @Published public private(set) var setupWarningMessage: String?
    @Published public private(set) var selectedRepo: OverviewRepoBucket?
    @Published public private(set) var selectedModel: OverviewModelName?
    @Published public private(set) var selectedHarness: OverviewHarnessName?
    @Published public private(set) var selectedSession: OverviewSessionRoute?
    @Published public private(set) var selectedPrompt: OverviewPromptRoute?
    @Published public var selectedRangePreset: OverviewRangePreset

    public var isTraceInspectorEnabled: Bool {
        traceInspector != nil
    }

    private let dashboard: OverviewDashboard
    private let traceInspector: TraceInspector?
    private let telemetrySetup: any HarnessTelemetrySetup
    private let launchAgent: DaemonLaunchAgent
    private let shouldRefreshLaunchAgentAfterStartupOverviewError: (any Error) -> Bool
    private let enablePostStartupOverviewRecovery: @Sendable () -> Void
    private let now: () -> Date
    private let calendar: Calendar
    private var overviewLoadID: UInt64 = 0
    private var traceInspectorLoadID: UInt64 = 0
    private var liveOverviewUpdates: Task<Void, Never>?

    public init(
        dashboard: OverviewDashboard,
        traceInspector: TraceInspector? = nil,
        telemetrySetup: any HarnessTelemetrySetup = NoOpHarnessTelemetrySetup(),
        launchAgent: DaemonLaunchAgent,
        shouldRefreshLaunchAgentAfterStartupOverviewError: @escaping (any Error) -> Bool = { _ in false },
        enablePostStartupOverviewRecovery: @escaping @Sendable () -> Void = {},
        selectedRangePreset: OverviewRangePreset = .lastSevenDays,
        now: @escaping () -> Date = Date.init,
        calendar: Calendar = .kvasirRollupUTC
    ) {
        self.dashboard = dashboard
        self.traceInspector = traceInspector
        self.telemetrySetup = telemetrySetup
        self.launchAgent = launchAgent
        self.shouldRefreshLaunchAgentAfterStartupOverviewError = shouldRefreshLaunchAgentAfterStartupOverviewError
        self.enablePostStartupOverviewRecovery = enablePostStartupOverviewRecovery
        self.selectedRangePreset = selectedRangePreset
        self.now = now
        self.calendar = calendar
    }

    public func start() async throws {
        do {
            try await telemetrySetup.ensureConfigured()
            setupWarningMessage = nil
        } catch {
            setupWarningMessage = error.localizedDescription
        }
        launchAgentOutcome = try launchAgent.ensureRegistered()
        do {
            try await refreshOverview()
            enablePostStartupOverviewRecovery()
        } catch {
            guard launchAgentOutcome != .requiresApproval,
                  shouldRefreshLaunchAgentAfterStartupOverviewError(error) else {
                throw error
            }
            launchAgentOutcome = try launchAgent.refreshRegistration()
            enablePostStartupOverviewRecovery()
            try await refreshOverview()
        }
    }

    public func selectRangePreset(_ preset: OverviewRangePreset) async throws {
        selectedRangePreset = preset
        try await refreshOverview(repo: selectedRepo, model: selectedModel, harness: selectedHarness, session: nil, prompt: nil) {
            selectedSession = nil
            selectedPrompt = nil
            clearTraceInspector()
        }
    }

    public func selectRepo(_ repo: OverviewRepoBucket?) async throws {
        try await refreshOverview(repo: repo, model: selectedModel, harness: selectedHarness, session: nil, prompt: nil) {
            selectedRepo = repo
            selectedSession = nil
            selectedPrompt = nil
            clearTraceInspector()
        }
    }

    public func selectModel(_ model: OverviewModelName?) async throws {
        try await refreshOverview(repo: selectedRepo, model: model, harness: selectedHarness, session: nil, prompt: nil) {
            selectedModel = model
            selectedSession = nil
            selectedPrompt = nil
            clearTraceInspector()
        }
    }

    public func selectHarness(_ harness: OverviewHarnessName?) async throws {
        try await refreshOverview(repo: selectedRepo, model: selectedModel, harness: harness, session: nil, prompt: nil) {
            selectedHarness = harness
            selectedSession = nil
            selectedPrompt = nil
            clearTraceInspector()
        }
    }

    public func drillDown(to target: OverviewDrillTarget) async throws {
        switch target {
        case .repo(let repo):
            try await selectRepo(repo)
        case .model(let model):
            try await selectModel(model)
        case .harness(let harness):
            try await selectHarness(harness)
        case .session(let session):
            try await refreshOverview(repo: selectedRepo, model: selectedModel, harness: session.harness, session: session, prompt: nil) {
                selectedHarness = session.harness
                selectedSession = session
                selectedPrompt = nil
                clearTraceInspector()
            }
        case .prompt(let prompt):
            try await refreshOverview(repo: selectedRepo, model: selectedModel, harness: prompt.session.harness, session: prompt.session, prompt: prompt) {
                selectedHarness = prompt.session.harness
                selectedSession = prompt.session
                selectedPrompt = prompt
                clearTraceInspector()
            }
            await refreshTraceInspector(for: prompt)
        }
    }

    public func clearSessionAndPrompt() async throws {
        try await refreshOverview(repo: selectedRepo, model: selectedModel, harness: selectedHarness, session: nil, prompt: nil) {
            selectedSession = nil
            selectedPrompt = nil
            clearTraceInspector()
        }
    }

    public func clearPrompt() async throws {
        try await refreshOverview(repo: selectedRepo, model: selectedModel, harness: selectedHarness, session: selectedSession, prompt: nil) {
            selectedPrompt = nil
            clearTraceInspector()
        }
    }

    public func refreshOverview() async throws {
        try await refreshOverview(
            repo: selectedRepo,
            model: selectedModel,
            harness: selectedHarness,
            session: selectedSession,
            prompt: selectedPrompt
        )
        if let selectedPrompt {
            await refreshTraceInspector(for: selectedPrompt)
        }
    }

    public func refreshTraceInspector() async {
        await refreshTraceInspector(for: selectedPrompt)
    }

    private func refreshOverview(
        repo: OverviewRepoBucket?,
        model: OverviewModelName?,
        harness: OverviewHarnessName?,
        session: OverviewSessionRoute? = nil,
        prompt: OverviewPromptRoute? = nil,
        beforeCommit: () -> Void = {}
    ) async throws {
        overviewLoadID += 1
        let loadID = overviewLoadID
        do {
            let snapshot = try await dashboard.load(
                range: selectedRangePreset.range(containing: now(), calendar: calendar),
                repo: repo,
                model: model,
                harness: harness,
                session: session,
                prompt: prompt
            )
            guard loadID == overviewLoadID else {
                return
            }
            beforeCommit()
            overviewSnapshot = snapshot
            errorMessage = nil
            startLiveOverviewUpdatesIfNeeded()
        } catch {
            guard loadID == overviewLoadID else {
                return
            }
            throw error
        }
    }

    public func record(error: any Error) {
        errorMessage = error.localizedDescription
    }

    private func refreshTraceInspector(for prompt: OverviewPromptRoute?) async {
        do {
            try await loadTraceInspector(for: prompt)
        } catch {
            // loadTraceInspector records failures on the scoped inspector error surface.
        }
    }

    private func loadTraceInspector(for prompt: OverviewPromptRoute?) async throws {
        traceInspectorLoadID += 1
        let loadID = traceInspectorLoadID
        guard let prompt, let traceInspector else {
            traceInspectorSnapshot = nil
            traceInspectorErrorMessage = nil
            return
        }

        do {
            let snapshot = try await traceInspector.load(prompt: prompt)
            guard loadID == traceInspectorLoadID else {
                return
            }
            traceInspectorSnapshot = snapshot
            traceInspectorErrorMessage = nil
        } catch {
            guard loadID == traceInspectorLoadID else {
                return
            }
            traceInspectorErrorMessage = error.localizedDescription
            throw error
        }
    }

    private func clearTraceInspector() {
        traceInspectorLoadID += 1
        traceInspectorSnapshot = nil
        traceInspectorErrorMessage = nil
    }

    private func startLiveOverviewUpdatesIfNeeded() {
        guard liveOverviewUpdates == nil else {
            return
        }
        liveOverviewUpdates = Task { [weak self, dashboard] in
            for await _ in dashboard.overviewRefreshEvents() {
                guard let self else {
                    return
                }
                guard !Task.isCancelled else {
                    return
                }
                do {
                    try await refreshOverview()
                } catch {
                    record(error: error)
                }
            }
        }
    }

    deinit {
        liveOverviewUpdates?.cancel()
    }
}

public protocol HarnessTelemetrySetup: Sendable {
    func ensureConfigured() async throws
}

public struct NoOpHarnessTelemetrySetup: HarnessTelemetrySetup {
    public init() {}

    public func ensureConfigured() async throws {}
}

public extension Calendar {
    static var kvasirRollupUTC: Calendar {
        var calendar = Calendar(identifier: .gregorian)
        calendar.timeZone = TimeZone(secondsFromGMT: 0)!
        return calendar
    }
}
