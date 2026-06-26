import Foundation

public struct OverviewTimeRange: Equatable, Sendable {
    public var start: Date
    public var end: Date

    public init(start: Date, end: Date) {
        self.start = start
        self.end = end
    }
}

public struct OverviewQuery: Equatable, Sendable {
    public var start: Date
    public var end: Date
    public var repo: OverviewRepoBucket?
    public var model: OverviewModelName?
    public var harness: OverviewHarnessName?
    public var session: OverviewSessionRoute?
    public var prompt: OverviewPromptRoute?

    public init(
        start: Date,
        end: Date,
        repo: OverviewRepoBucket? = nil,
        model: OverviewModelName? = nil,
        harness: OverviewHarnessName? = nil,
        session: OverviewSessionRoute? = nil,
        prompt: OverviewPromptRoute? = nil
    ) {
        self.start = start
        self.end = end
        self.repo = repo
        self.model = model
        self.harness = harness
        self.session = session
        self.prompt = prompt
    }
}

public struct OverviewModelName: Hashable, Comparable, Sendable {
    private let value: String

    public init(_ value: String) {
        self.value = value
    }

    public func displayName() -> String {
        value
    }

    public static func < (lhs: Self, rhs: Self) -> Bool {
        lhs.value < rhs.value
    }
}

public struct OverviewRepoName: Hashable, Comparable, Sendable {
    public var rawValue: String

    public init(_ rawValue: String) {
        self.rawValue = rawValue
    }

    public static func < (lhs: Self, rhs: Self) -> Bool {
        lhs.rawValue < rhs.rawValue
    }
}

public struct OverviewHarnessName: Hashable, Comparable, Sendable {
    private let value: String

    public init(_ value: String) {
        self.value = value
    }

    public func displayName() -> String {
        value
    }

    public static func < (lhs: Self, rhs: Self) -> Bool {
        lhs.value < rhs.value
    }
}

public struct OverviewSessionID: Hashable, Comparable, Sendable {
    private let value: String

    public init(_ value: String) {
        self.value = value
    }

    public func displayName() -> String {
        value
    }

    public static func < (lhs: Self, rhs: Self) -> Bool {
        lhs.value < rhs.value
    }
}

public struct OverviewPromptID: Hashable, Comparable, Sendable {
    private let value: String

    public init(_ value: String) {
        self.value = value
    }

    public func displayName() -> String {
        value
    }

    public static func < (lhs: Self, rhs: Self) -> Bool {
        lhs.value < rhs.value
    }
}

public struct OverviewSessionRoute: Hashable, Sendable {
    public var harness: OverviewHarnessName
    public var sessionID: OverviewSessionID

    public init(harness: OverviewHarnessName, sessionID: OverviewSessionID) {
        self.harness = harness
        self.sessionID = sessionID
    }
}

public struct OverviewPromptRoute: Hashable, Sendable {
    public var session: OverviewSessionRoute
    public var promptID: OverviewPromptID

    public init(session: OverviewSessionRoute, promptID: OverviewPromptID) {
        self.session = session
        self.promptID = promptID
    }
}

public enum OverviewDrillTarget: Equatable, Sendable {
    case repo(OverviewRepoBucket)
    case model(OverviewModelName)
    case harness(OverviewHarnessName)
    case session(OverviewSessionRoute)
    case prompt(OverviewPromptRoute)
}

public struct OverviewRepoPath: Hashable, Comparable, Sendable {
    public var rawValue: String

    public init(_ rawValue: String) {
        self.rawValue = rawValue
    }

    public static func < (lhs: Self, rhs: Self) -> Bool {
        lhs.rawValue < rhs.rawValue
    }
}

public struct OverviewRepoIdentity: Hashable, Comparable, Sendable {
    public let name: OverviewRepoName?
    public let path: OverviewRepoPath?

    public init?(name: OverviewRepoName?, path: OverviewRepoPath?) {
        guard name != nil || path != nil else {
            return nil
        }
        self.name = name
        self.path = path
    }

    public static func < (lhs: Self, rhs: Self) -> Bool {
        (lhs.name?.rawValue ?? "", lhs.path?.rawValue ?? "") <
            (rhs.name?.rawValue ?? "", rhs.path?.rawValue ?? "")
    }
}

public enum OverviewRepoBucket: Hashable, Comparable, Sendable {
    case noRepo
    case repo(OverviewRepoIdentity)

    public static func < (lhs: Self, rhs: Self) -> Bool {
        lhs.sortKey < rhs.sortKey
    }

    public var displayName: String {
        switch self {
        case .noRepo:
            return "<no-repo>"
        case .repo(let identity):
            return identity.name?.rawValue ?? identity.path?.rawValue ?? "Unknown repo"
        }
    }

    private var sortKey: String {
        switch self {
        case .noRepo:
            return "\u{10ffff}"
        case .repo(let identity):
            return identity.name?.rawValue ?? identity.path?.rawValue ?? ""
        }
    }
}

public struct OverviewRollupDay: Comparable, Hashable, Sendable {
    public var year: Int
    public var month: Int
    public var day: Int

    public init(year: Int, month: Int, day: Int) {
        self.year = year
        self.month = month
        self.day = day
    }

    public static func < (lhs: Self, rhs: Self) -> Bool {
        (lhs.year, lhs.month, lhs.day) < (rhs.year, rhs.month, rhs.day)
    }
}

public enum OverviewCostSource: Equatable, Sendable {
    case native
    case estimated
    case mixed

    public var estimateLabel: String? {
        switch self {
        case .native:
            return nil
        case .estimated:
            return "Estimated"
        case .mixed:
            return "Partly estimated"
        }
    }

    public var chartMarkerLabel: String? {
        switch self {
        case .native:
            return nil
        case .estimated:
            return "Est."
        case .mixed:
            return "Partly est."
        }
    }

    public var usesEstimatedCost: Bool {
        switch self {
        case .native:
            return false
        case .estimated, .mixed:
            return true
        }
    }
}

public struct OverviewCostDisplay: Equatable, Sendable {
    public static let estimateBadgeSystemImage = "plusminus"

    public var costUsdNanos: UInt64
    public var source: OverviewCostSource?

    public init(costUsdNanos: UInt64, source: OverviewCostSource?) {
        self.costUsdNanos = costUsdNanos
        self.source = source
    }

    public var estimateLabel: String? {
        source?.estimateLabel
    }

    public var chartMarkerLabel: String? {
        source?.chartMarkerLabel
    }

    public var usesEstimatedCost: Bool {
        source?.usesEstimatedCost ?? false
    }
}

public struct OverviewCostDashboardPresentation: Equatable, Sendable {
    public var total: OverviewCostDisplay
    public var series: [OverviewCostDisplay]
    public var repos: [OverviewCostDisplay]
    public var models: [OverviewCostDisplay]
    public var harnesses: [OverviewCostDisplay]

    public init(
        total: OverviewCostDisplay,
        series: [OverviewCostDisplay],
        repos: [OverviewCostDisplay],
        models: [OverviewCostDisplay],
        harnesses: [OverviewCostDisplay] = []
    ) {
        self.total = total
        self.series = series
        self.repos = repos
        self.models = models
        self.harnesses = harnesses
    }
}

public struct OverviewFilterBarPresentation: Equatable, Sendable {
    public var repo: String?
    public var model: String?
    public var harness: String?
    public var session: String?
    public var prompt: String?
    public var dimensions: [OverviewDimensionFilterPresentation]

    public init(
        repo: String? = nil,
        model: String? = nil,
        harness: String? = nil,
        session: String? = nil,
        prompt: String? = nil,
        dimensions: [OverviewDimensionFilterPresentation] = []
    ) {
        self.repo = repo
        self.model = model
        self.harness = harness
        self.session = session
        self.prompt = prompt
        self.dimensions = dimensions
    }
}

public enum OverviewDimensionKind: Equatable, Sendable {
    case subagent
    case skill
    case plugin
    case mcpServer
    case mcpTool
    case effort
    case speed
    case querySource
    case accountOrg

    public var title: String {
        switch self {
        case .subagent:
            return "Subagent"
        case .skill:
            return "Skill"
        case .plugin:
            return "Plugin"
        case .mcpServer:
            return "MCP server"
        case .mcpTool:
            return "MCP tool"
        case .effort:
            return "Effort"
        case .speed:
            return "Speed"
        case .querySource:
            return "Query source"
        case .accountOrg:
            return "Account/org"
        }
    }
}

public struct OverviewDimensionValue: Hashable, Comparable, Sendable {
    private let value: String

    public init(_ value: String) {
        self.value = value
    }

    public func displayName() -> String {
        value
    }

    public static func < (lhs: Self, rhs: Self) -> Bool {
        lhs.value < rhs.value
    }
}

public struct OverviewDimensionFilter: Equatable, Sendable {
    public var kind: OverviewDimensionKind
    public var value: OverviewDimensionValue

    public init(kind: OverviewDimensionKind, value: OverviewDimensionValue) {
        self.kind = kind
        self.value = value
    }

    public var presentation: OverviewDimensionFilterPresentation {
        OverviewDimensionFilterPresentation(title: kind.title, value: value.displayName())
    }
}

public struct OverviewDimensionFilterPresentation: Equatable, Sendable {
    public var title: String
    public var value: String

    public init(title: String, value: String) {
        self.title = title
        self.value = value
    }
}

public struct OverviewTotals: Equatable, Sendable {
    public var totalTokens: UInt64
    public var costUsdNanos: UInt64
    public var costSource: OverviewCostSource?
    public var toolCalls: UInt64

    public init(
        totalTokens: UInt64,
        costUsdNanos: UInt64,
        costSource: OverviewCostSource? = nil,
        toolCalls: UInt64
    ) {
        self.totalTokens = totalTokens
        self.costUsdNanos = costUsdNanos
        self.costSource = costSource
        self.toolCalls = toolCalls
    }

    public var costDisplay: OverviewCostDisplay {
        OverviewCostDisplay(costUsdNanos: costUsdNanos, source: costSource)
    }
}

public struct OverviewSeriesPoint: Equatable, Sendable {
    public var day: OverviewRollupDay
    public var totalTokens: UInt64
    public var costUsdNanos: UInt64
    public var costSource: OverviewCostSource?
    public var toolCalls: UInt64

    public init(
        day: OverviewRollupDay,
        totalTokens: UInt64,
        costUsdNanos: UInt64,
        costSource: OverviewCostSource? = nil,
        toolCalls: UInt64
    ) {
        self.day = day
        self.totalTokens = totalTokens
        self.costUsdNanos = costUsdNanos
        self.costSource = costSource
        self.toolCalls = toolCalls
    }

    public var costDisplay: OverviewCostDisplay {
        OverviewCostDisplay(costUsdNanos: costUsdNanos, source: costSource)
    }
}

public struct OverviewSnapshot: Equatable, Sendable {
    public var totals: OverviewTotals
    public var series: [OverviewSeriesPoint]
    public var repoBreakdown: [OverviewRepoSummary]
    public var modelBreakdown: [OverviewModelSummary]
    public var harnessBreakdown: [OverviewHarnessSummary]
    public var sessionBreakdown: [OverviewSessionSummary]
    public var sessionBreakdownMoreAvailable: UInt64
    public var promptBreakdown: [OverviewPromptSummary]
    public var promptBreakdownMoreAvailable: UInt64
    public var selectedRepo: OverviewRepoBucket?
    public var selectedHarness: OverviewHarnessName?
    public var selectedModel: OverviewModelName?
    public var selectedSession: OverviewSessionRoute?
    public var selectedPrompt: OverviewPromptRoute?
    public var dimensions: [OverviewDimensionFilter]

    public init(
        totals: OverviewTotals,
        series: [OverviewSeriesPoint],
        repoBreakdown: [OverviewRepoSummary],
        modelBreakdown: [OverviewModelSummary] = [],
        harnessBreakdown: [OverviewHarnessSummary] = [],
        sessionBreakdown: [OverviewSessionSummary] = [],
        sessionBreakdownMoreAvailable: UInt64 = 0,
        promptBreakdown: [OverviewPromptSummary] = [],
        promptBreakdownMoreAvailable: UInt64 = 0,
        selectedRepo: OverviewRepoBucket?,
        selectedHarness: OverviewHarnessName? = nil,
        selectedModel: OverviewModelName? = nil,
        selectedSession: OverviewSessionRoute? = nil,
        selectedPrompt: OverviewPromptRoute? = nil,
        dimensions: [OverviewDimensionFilter] = []
    ) {
        self.totals = totals
        self.series = series
        self.repoBreakdown = repoBreakdown
        self.modelBreakdown = modelBreakdown
        self.harnessBreakdown = harnessBreakdown
        self.sessionBreakdown = sessionBreakdown
        self.sessionBreakdownMoreAvailable = sessionBreakdownMoreAvailable
        self.promptBreakdown = promptBreakdown
        self.promptBreakdownMoreAvailable = promptBreakdownMoreAvailable
        self.selectedRepo = selectedRepo
        self.selectedHarness = selectedHarness
        self.selectedModel = selectedModel
        self.selectedSession = selectedSession
        self.selectedPrompt = selectedPrompt
        self.dimensions = dimensions
    }

    public var costDashboardPresentation: OverviewCostDashboardPresentation {
        OverviewCostDashboardPresentation(
            total: totals.costDisplay,
            series: series.map(\.costDisplay),
            repos: repoBreakdown.map(\.totals.costDisplay),
            models: modelBreakdown.map(\.totals.costDisplay),
            harnesses: harnessBreakdown.map(\.totals.costDisplay)
        )
    }

    public var filterBarPresentation: OverviewFilterBarPresentation {
        let selectedHarness =
            selectedPrompt?.session.harness ?? selectedSession?.harness ?? self.selectedHarness
        let selectedSessionID = selectedPrompt?.session.sessionID ?? selectedSession?.sessionID
        return OverviewFilterBarPresentation(
            repo: selectedRepo?.displayName,
            model: selectedModel?.displayName(),
            harness: selectedHarness?.displayName(),
            session: selectedSessionID?.displayName(),
            prompt: selectedPrompt?.promptID.displayName(),
            dimensions: dimensions.map(\.presentation)
        )
    }
}

public struct OverviewHarnessSummary: Equatable, Sendable {
    public var harness: OverviewHarnessName
    public var totals: OverviewTotals
    public var lastActivity: Date

    public init(
        harness: OverviewHarnessName,
        totals: OverviewTotals,
        lastActivity: Date = Date(timeIntervalSince1970: 0)
    ) {
        self.harness = harness
        self.totals = totals
        self.lastActivity = lastActivity
    }
}

public struct OverviewRepoSummary: Equatable, Sendable {
    public var repo: OverviewRepoBucket
    public var totals: OverviewTotals

    public init(repo: OverviewRepoBucket, totals: OverviewTotals) {
        self.repo = repo
        self.totals = totals
    }
}

public struct OverviewModelSummary: Equatable, Sendable {
    public var model: OverviewModelName
    public var totals: OverviewTotals

    public init(model: OverviewModelName, totals: OverviewTotals) {
        self.model = model
        self.totals = totals
    }
}

public enum OverviewAttributionStatus: Equatable, Sendable {
    case direct
    case traceDerived
    case partial
    case unavailable

    public var displayName: String {
        switch self {
        case .direct:
            return "Direct"
        case .traceDerived:
            return "Trace"
        case .partial:
            return "Partial"
        case .unavailable:
            return "Unavailable"
        }
    }
}

public struct OverviewSessionSummary: Equatable, Sendable {
    public var route: OverviewSessionRoute
    public var totals: OverviewTotals
    public var attributionStatus: OverviewAttributionStatus
    public var lastActivity: Date

    public init(
        route: OverviewSessionRoute,
        totals: OverviewTotals,
        attributionStatus: OverviewAttributionStatus = .direct,
        lastActivity: Date = Date(timeIntervalSince1970: 0)
    ) {
        self.route = route
        self.totals = totals
        self.attributionStatus = attributionStatus
        self.lastActivity = lastActivity
    }
}

public struct OverviewPromptSummary: Equatable, Sendable {
    public var route: OverviewPromptRoute
    public var totals: OverviewTotals
    public var attributionStatus: OverviewAttributionStatus
    public var lastActivity: Date

    public init(
        route: OverviewPromptRoute,
        totals: OverviewTotals,
        attributionStatus: OverviewAttributionStatus = .direct,
        lastActivity: Date = Date(timeIntervalSince1970: 0)
    ) {
        self.route = route
        self.totals = totals
        self.attributionStatus = attributionStatus
        self.lastActivity = lastActivity
    }
}

public protocol OverviewClient: Sendable {
    func loadOverviewSnapshot(query: OverviewQuery) async throws -> OverviewSnapshot
}

public struct OverviewDashboard: Sendable {
    private let client: any OverviewClient

    public init(client: any OverviewClient) {
        self.client = client
    }

    public func load(
        range: OverviewTimeRange,
        repo: OverviewRepoBucket? = nil,
        model: OverviewModelName? = nil,
        harness: OverviewHarnessName? = nil,
        session: OverviewSessionRoute? = nil,
        prompt: OverviewPromptRoute? = nil
    ) async throws -> OverviewSnapshot {
        let query = OverviewQuery(
            start: range.start,
            end: range.end,
            repo: repo,
            model: model,
            harness: harness,
            session: session,
            prompt: prompt
        )
        return try await client.loadOverviewSnapshot(query: query)
    }
}
