import Charts
import SwiftUI
import KvasirViewerCore

struct OverviewScreen: View {
    @ObservedObject var model: KvasirViewerModel

    var body: some View {
        VStack(spacing: 0) {
            header
            Divider()
            content
        }
        .frame(minWidth: 920, minHeight: 620)
        .background(Color(nsColor: .windowBackgroundColor))
    }

    private var header: some View {
        HStack(spacing: 12) {
            Text("Overview")
                .font(.title2.weight(.semibold))

            Spacer()

            Text("UTC")
                .font(.caption.weight(.medium))
                .foregroundStyle(.secondary)

            rangePicker

            Button {
                Task {
                    do {
                        try await model.refreshOverview()
                    } catch {
                        model.record(error: error)
                    }
                }
            } label: {
                Label("Refresh", systemImage: "arrow.clockwise")
            }
        }
        .padding(.horizontal, 24)
        .padding(.vertical, 16)
    }

    @ViewBuilder
    private var rangePicker: some View {
        let picker = Picker("Time range", selection: $model.selectedRangePreset) {
            ForEach(OverviewRangePreset.allCases) { preset in
                Text(preset.label).tag(preset)
            }
        }
        .pickerStyle(.segmented)
        .frame(width: 260)

        if #available(macOS 14.0, *) {
            picker.onChange(of: model.selectedRangePreset) { _, preset in
                refreshRange(preset)
            }
        } else {
            picker.onChange(of: model.selectedRangePreset) { preset in
                refreshRange(preset)
            }
        }
    }

    @ViewBuilder
    private var content: some View {
        if let snapshot = model.overviewSnapshot {
            ScrollView {
                VStack(alignment: .leading, spacing: 24) {
                    if model.launchAgentOutcome == .requiresApproval {
                        approvalBanner
                    }
                    if let setupWarningMessage = model.setupWarningMessage {
                        warningBanner(setupWarningMessage)
                    }
                    if let errorMessage = model.errorMessage {
                        errorBanner(errorMessage)
                    }
                    totals(snapshot.totals)
                    charts(snapshot.series)
                }
                .padding(24)
            }
        } else {
            VStack(spacing: 12) {
                ProgressView()
                if model.launchAgentOutcome == .requiresApproval {
                    approvalBanner
                        .frame(maxWidth: 520)
                }
                if let setupWarningMessage = model.setupWarningMessage {
                    warningBanner(setupWarningMessage)
                        .frame(maxWidth: 520)
                }
                Text(model.errorMessage ?? "Loading overview")
                    .foregroundStyle(.secondary)
            }
            .frame(maxWidth: .infinity, maxHeight: .infinity)
        }
    }

    private func totals(_ totals: OverviewTotals) -> some View {
        HStack(spacing: 12) {
            TotalTile(title: "Tokens", value: totals.totalTokens.formatted(), systemImage: "sum")
            TotalTile(title: "Cost", value: usd(totals.costUsdNanos), systemImage: "dollarsign")
            TotalTile(title: "Tool calls", value: totals.toolCalls.formatted(), systemImage: "hammer")
        }
    }

    private func charts(_ series: [OverviewSeriesPoint]) -> some View {
        Grid(horizontalSpacing: 16, verticalSpacing: 16) {
            GridRow {
                MetricChart(title: "Tokens", series: series, color: .teal) { $0.totalTokens }
                MetricChart(title: "Cost", series: series, color: .indigo) { $0.costUsdNanos }
            }
            GridRow {
                MetricChart(title: "Tool calls", series: series, color: .orange) { $0.toolCalls }
                    .gridCellColumns(2)
            }
        }
    }

    private func errorBanner(_ message: String) -> some View {
        Label(message, systemImage: "exclamationmark.triangle")
            .foregroundStyle(.red)
            .padding(10)
            .frame(maxWidth: .infinity, alignment: .leading)
            .background(.red.opacity(0.08), in: RoundedRectangle(cornerRadius: 8))
    }

    private var approvalBanner: some View {
        Label("Daemon requires approval in System Settings", systemImage: "person.crop.circle.badge.exclamationmark")
            .foregroundStyle(.orange)
            .padding(10)
            .frame(maxWidth: .infinity, alignment: .leading)
            .background(.orange.opacity(0.08), in: RoundedRectangle(cornerRadius: 8))
    }

    private func warningBanner(_ message: String) -> some View {
        Label(message, systemImage: "exclamationmark.triangle")
            .foregroundStyle(.orange)
            .padding(10)
            .frame(maxWidth: .infinity, alignment: .leading)
            .background(.orange.opacity(0.08), in: RoundedRectangle(cornerRadius: 8))
    }

    private func usd(_ nanos: UInt64) -> String {
        let dollars = Decimal(nanos) / Decimal(1_000_000_000)
        return dollars.formatted(.currency(code: "USD").precision(.fractionLength(2...6)))
    }

    private func refreshRange(_ preset: OverviewRangePreset) {
        Task {
            do {
                try await model.selectRangePreset(preset)
            } catch {
                model.record(error: error)
            }
        }
    }
}

private struct TotalTile: View {
    let title: String
    let value: String
    let systemImage: String

    var body: some View {
        VStack(alignment: .leading, spacing: 8) {
            Label(title, systemImage: systemImage)
                .font(.caption.weight(.medium))
                .foregroundStyle(.secondary)
            Text(value)
                .font(.system(.title2, design: .rounded, weight: .semibold))
                .monospacedDigit()
        }
        .frame(maxWidth: .infinity, alignment: .leading)
        .padding(14)
        .background(Color(nsColor: .controlBackgroundColor), in: RoundedRectangle(cornerRadius: 8))
        .overlay(
            RoundedRectangle(cornerRadius: 8)
                .stroke(Color(nsColor: .separatorColor).opacity(0.45))
        )
    }
}

private struct MetricChart: View {
    let title: String
    let series: [OverviewSeriesPoint]
    let color: Color
    let value: (OverviewSeriesPoint) -> UInt64

    var body: some View {
        VStack(alignment: .leading, spacing: 12) {
            Text(title)
                .font(.headline)

            Chart(series, id: \.day) { point in
                BarMark(
                    x: .value("Day", point.day.shortLabel),
                    y: .value(title, value(point))
                )
                .foregroundStyle(color.gradient)
            }
            .chartYAxis {
                AxisMarks(position: .leading)
            }
            .frame(height: 190)
        }
        .padding(14)
        .background(Color(nsColor: .textBackgroundColor), in: RoundedRectangle(cornerRadius: 8))
        .overlay(
            RoundedRectangle(cornerRadius: 8)
                .stroke(Color(nsColor: .separatorColor).opacity(0.35))
        )
    }
}

private extension OverviewRollupDay {
    var shortLabel: String {
        "\(month)/\(day)"
    }
}
