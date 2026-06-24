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
    @Published public private(set) var launchAgentOutcome: LaunchAgentRegistrationOutcome?
    @Published public private(set) var errorMessage: String?
    @Published public private(set) var setupWarningMessage: String?
    @Published public var selectedRangePreset: OverviewRangePreset

    private let dashboard: OverviewDashboard
    private let telemetrySetup: any HarnessTelemetrySetup
    private let launchAgent: DaemonLaunchAgent
    private let shouldRefreshLaunchAgentAfterStartupOverviewError: (any Error) -> Bool
    private let enablePostStartupOverviewRecovery: @Sendable () -> Void
    private let now: () -> Date
    private let calendar: Calendar
    private var overviewLoadID: UInt64 = 0

    public init(
        dashboard: OverviewDashboard,
        telemetrySetup: any HarnessTelemetrySetup = NoOpHarnessTelemetrySetup(),
        launchAgent: DaemonLaunchAgent,
        shouldRefreshLaunchAgentAfterStartupOverviewError: @escaping (any Error) -> Bool = { _ in false },
        enablePostStartupOverviewRecovery: @escaping @Sendable () -> Void = {},
        selectedRangePreset: OverviewRangePreset = .lastSevenDays,
        now: @escaping () -> Date = Date.init,
        calendar: Calendar = .kvasirRollupUTC
    ) {
        self.dashboard = dashboard
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
        try await refreshOverview()
    }

    public func refreshOverview() async throws {
        overviewLoadID += 1
        let loadID = overviewLoadID
        do {
            let snapshot = try await dashboard.load(
                range: selectedRangePreset.range(containing: now(), calendar: calendar)
            )
            guard loadID == overviewLoadID else {
                return
            }
            overviewSnapshot = snapshot
            errorMessage = nil
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
