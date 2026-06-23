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
    @Published public var selectedRangePreset: OverviewRangePreset

    private let dashboard: OverviewDashboard
    private let launchAgent: DaemonLaunchAgent
    private let now: () -> Date
    private let calendar: Calendar
    private var overviewLoadID: UInt64 = 0

    public init(
        dashboard: OverviewDashboard,
        launchAgent: DaemonLaunchAgent,
        selectedRangePreset: OverviewRangePreset = .lastSevenDays,
        now: @escaping () -> Date = Date.init,
        calendar: Calendar = .kvasirRollupUTC
    ) {
        self.dashboard = dashboard
        self.launchAgent = launchAgent
        self.selectedRangePreset = selectedRangePreset
        self.now = now
        self.calendar = calendar
    }

    public func start() async throws {
        launchAgentOutcome = try launchAgent.ensureRegistered()
        try await refreshOverview()
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

public extension Calendar {
    static var kvasirRollupUTC: Calendar {
        var calendar = Calendar(identifier: .gregorian)
        calendar.timeZone = TimeZone(secondsFromGMT: 0)!
        return calendar
    }
}
