import Foundation

public struct TraceInspectorQuery: Equatable, Sendable {
    public var prompt: OverviewPromptRoute

    public init(prompt: OverviewPromptRoute) {
        self.prompt = prompt
    }
}

public struct TraceInspectorSnapshot: Equatable, Sendable {
    public var prompt: OverviewPromptRoute
    public var traces: [TraceInspectorTrace]
    public var content: [TraceInspectorContentItem]
    public var contentAvailability: TraceInspectorContentAvailability

    public init(
        prompt: OverviewPromptRoute,
        traces: [TraceInspectorTrace],
        content: [TraceInspectorContentItem],
        contentAvailability: TraceInspectorContentAvailability
    ) {
        self.prompt = prompt
        self.traces = traces
        self.content = content
        self.contentAvailability = contentAvailability
    }
}

public struct TraceInspectorTrace: Equatable, Sendable {
    public var traceID: TraceInspectorTraceID
    public var spans: [TraceInspectorSpan]
    public var durations: TraceInspectorDurations

    public init(
        traceID: TraceInspectorTraceID,
        spans: [TraceInspectorSpan],
        durations: TraceInspectorDurations
    ) {
        self.traceID = traceID
        self.spans = spans
        self.durations = durations
    }

    public var waterfallRows: [TraceInspectorWaterfallRow] {
        guard let traceStart = spans.map(\.startedAt).min(),
              let traceEnd = spans.map(\.endedAt).max()
        else {
            return []
        }
        let totalSeconds = max(traceEnd.timeIntervalSince(traceStart), .leastNonzeroMagnitude)
        let spansByID = Dictionary(spans.map { ($0.spanID, $0) }, uniquingKeysWith: { first, _ in first })

        return spans.map { span in
            TraceInspectorWaterfallRow(
                span: span,
                depth: waterfallDepth(for: span, spansByID: spansByID),
                startFraction: span.startedAt.timeIntervalSince(traceStart) / totalSeconds,
                widthFraction: max(span.endedAt.timeIntervalSince(span.startedAt), 0) / totalSeconds
            )
        }
    }

    private func waterfallDepth(
        for span: TraceInspectorSpan,
        spansByID: [TraceInspectorSpanID: TraceInspectorSpan]
    ) -> Int {
        var depth = 0
        var parentSpanID = span.parentSpanID
        var seen = Set<TraceInspectorSpanID>()
        while let currentParentID = parentSpanID,
              seen.insert(currentParentID).inserted,
              let parentSpan = spansByID[currentParentID] {
            depth += 1
            parentSpanID = parentSpan.parentSpanID
        }
        return depth
    }
}

public struct TraceInspectorWaterfallRow: Equatable, Sendable {
    public var span: TraceInspectorSpan
    public var depth: Int
    public var startFraction: Double
    public var widthFraction: Double

    public init(
        span: TraceInspectorSpan,
        depth: Int,
        startFraction: Double,
        widthFraction: Double
    ) {
        self.span = span
        self.depth = depth
        self.startFraction = startFraction
        self.widthFraction = widthFraction
    }
}

public struct TraceInspectorTraceID: Hashable, Sendable {
    private let value: String

    public init(_ value: String) {
        self.value = value
    }

    public func displayName() -> String {
        value
    }
}

public struct TraceInspectorSpanID: Hashable, Sendable {
    private let value: String

    public init(_ value: String) {
        self.value = value
    }

    public func displayName() -> String {
        value
    }
}

public struct TraceInspectorSpanName: Hashable, Sendable {
    private let value: String

    public init(_ value: String) {
        self.value = value
    }

    public func displayName() -> String {
        value
    }
}

public struct TraceInspectorToolName: Hashable, Sendable {
    private let value: String

    public init(_ value: String) {
        self.value = value
    }

    public func displayName() -> String {
        value
    }
}

public struct TraceInspectorSpan: Equatable, Sendable {
    public var spanID: TraceInspectorSpanID
    public var parentSpanID: TraceInspectorSpanID?
    public var kind: TraceInspectorSpanKind
    public var name: TraceInspectorSpanName
    public var startedAt: Date
    public var endedAt: Date
    public var durationMilliseconds: UInt64
    public var toolName: TraceInspectorToolName?

    public init(
        spanID: TraceInspectorSpanID,
        parentSpanID: TraceInspectorSpanID?,
        kind: TraceInspectorSpanKind,
        name: TraceInspectorSpanName,
        startedAt: Date,
        endedAt: Date,
        durationMilliseconds: UInt64,
        toolName: TraceInspectorToolName?
    ) {
        self.spanID = spanID
        self.parentSpanID = parentSpanID
        self.kind = kind
        self.name = name
        self.startedAt = startedAt
        self.endedAt = endedAt
        self.durationMilliseconds = durationMilliseconds
        self.toolName = toolName
    }
}

public enum TraceInspectorSpanKind: Equatable, Sendable {
    case interaction
    case llmRequest
    case toolCall

    public var displayName: String {
        switch self {
        case .interaction:
            return "Interaction"
        case .llmRequest:
            return "Request"
        case .toolCall:
            return "Tool"
        }
    }
}

public struct TraceInspectorDurations: Equatable, Sendable {
    public var timeToFirstTokenMilliseconds: UInt64?
    public var requestMilliseconds: UInt64?
    public var toolMilliseconds: UInt64?

    public init(
        timeToFirstTokenMilliseconds: UInt64?,
        requestMilliseconds: UInt64?,
        toolMilliseconds: UInt64?
    ) {
        self.timeToFirstTokenMilliseconds = timeToFirstTokenMilliseconds
        self.requestMilliseconds = requestMilliseconds
        self.toolMilliseconds = toolMilliseconds
    }
}

public struct TraceInspectorContentItem: Equatable, Sendable {
    public var occurredAt: Date
    public var harness: OverviewHarnessName
    public var kind: TraceInspectorContentKind
    public var content: TraceInspectorContentText

    public init(
        occurredAt: Date,
        harness: OverviewHarnessName,
        kind: TraceInspectorContentKind,
        content: TraceInspectorContentText
    ) {
        self.occurredAt = occurredAt
        self.harness = harness
        self.kind = kind
        self.content = content
    }
}

public enum TraceInspectorContentKind: Equatable, Sendable {
    case userPrompt
    case assistantMessage
    case toolInput
    case toolOutput
    case rawApiRequest
    case rawApiResponse

    public var displayName: String {
        switch self {
        case .userPrompt:
            return "Prompt"
        case .assistantMessage:
            return "Model"
        case .toolInput:
            return "Tool input"
        case .toolOutput:
            return "Tool output"
        case .rawApiRequest:
            return "API request"
        case .rawApiResponse:
            return "API response"
        }
    }
}

public struct TraceInspectorContentText: Equatable, Sendable {
    private let value: String

    public init(_ value: String) {
        self.value = value
    }

    public func displayText() -> String {
        value
    }
}

public enum TraceInspectorContentAvailability: Equatable, Sendable {
    case captured(
        harness: OverviewHarnessName,
        kinds: [TraceInspectorContentKindAvailability]
    )
    case unavailable(reason: TraceInspectorContentUnavailableReason)
}

public enum TraceInspectorContentKindAvailability: Equatable, Sendable {
    case captured(TraceInspectorContentKind)
    case unavailable(kind: TraceInspectorContentKind, reason: TraceInspectorContentUnavailableReason)
}

public enum TraceInspectorContentUnavailableReason: Equatable, Sendable {
    case notProvidedByHarness
    case notCapturedForPrompt
    case promptNotFound
}

public protocol TraceInspectorClient: Sendable {
    func loadTraceInspector(query: TraceInspectorQuery) async throws -> TraceInspectorSnapshot
}

public struct TraceInspector: Sendable {
    private let client: any TraceInspectorClient

    public init(client: any TraceInspectorClient) {
        self.client = client
    }

    public func load(prompt: OverviewPromptRoute) async throws -> TraceInspectorSnapshot {
        try await client.loadTraceInspector(query: TraceInspectorQuery(prompt: prompt))
    }
}
