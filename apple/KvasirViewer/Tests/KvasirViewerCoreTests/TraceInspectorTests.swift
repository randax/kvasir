import Foundation
import Testing

@testable import KvasirViewerCore

@Test
func traceInspectorBuildsWaterfallRowsFromSpanTimingAndParentage() {
    let trace = TraceInspectorTrace(
        traceID: TraceInspectorTraceID("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"),
        spans: [
            TraceInspectorSpan(
                spanID: TraceInspectorSpanID("root"),
                parentSpanID: nil,
                kind: .interaction,
                name: TraceInspectorSpanName("opencode.interaction"),
                startedAt: Date(timeIntervalSince1970: 100),
                endedAt: Date(timeIntervalSince1970: 104),
                durationMilliseconds: 4_000,
                toolName: nil
            ),
            TraceInspectorSpan(
                spanID: TraceInspectorSpanID("request"),
                parentSpanID: TraceInspectorSpanID("root"),
                kind: .llmRequest,
                name: TraceInspectorSpanName("opencode.generate_text"),
                startedAt: Date(timeIntervalSince1970: 101),
                endedAt: Date(timeIntervalSince1970: 103),
                durationMilliseconds: 2_000,
                toolName: nil
            ),
            TraceInspectorSpan(
                spanID: TraceInspectorSpanID("tool"),
                parentSpanID: TraceInspectorSpanID("request"),
                kind: .toolCall,
                name: TraceInspectorSpanName("opencode.tool"),
                startedAt: Date(timeIntervalSince1970: 102),
                endedAt: Date(timeIntervalSince1970: 102.5),
                durationMilliseconds: 500,
                toolName: TraceInspectorToolName("Read")
            )
        ],
        durations: TraceInspectorDurations(
            timeToFirstTokenMilliseconds: 250,
            requestMilliseconds: 2_000,
            toolMilliseconds: 500
        )
    )

    #expect(trace.waterfallRows == [
        TraceInspectorWaterfallRow(
            span: trace.spans[0],
            depth: 0,
            startFraction: 0,
            widthFraction: 1
        ),
        TraceInspectorWaterfallRow(
            span: trace.spans[1],
            depth: 1,
            startFraction: 0.25,
            widthFraction: 0.5
        ),
        TraceInspectorWaterfallRow(
            span: trace.spans[2],
            depth: 2,
            startFraction: 0.5,
            widthFraction: 0.125
        ),
    ])
}
