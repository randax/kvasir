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
                    repoDashboard(snapshot.repoBreakdown)
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

    private func repoDashboard(_ repos: [OverviewRepoSummary]) -> some View {
        VStack(alignment: .leading, spacing: 10) {
            HStack(spacing: 12) {
                Text("Repos")
                    .font(.headline)

                if let selectedRepo = model.selectedRepo {
                    Text(selectedRepo.displayName)
                        .font(.caption.weight(.medium))
                        .foregroundStyle(.secondary)
                        .lineLimit(1)
                }

                Spacer()

                Button {
                    selectRepo(nil)
                } label: {
                    Label("All repos", systemImage: "square.grid.2x2")
                }
                .disabled(model.selectedRepo == nil)
            }

            VStack(spacing: 0) {
                if repos.isEmpty {
                    RepoEmptyRow()
                } else {
                    RepoHeaderRow()
                    ForEach(repos, id: \.repo) { summary in
                        RepoSummaryRow(
                            summary: summary,
                            isSelected: model.selectedRepo == summary.repo,
                            costFormatter: usd
                        ) {
                            selectRepo(summary.repo)
                        }
                        Divider()
                    }
                }
            }
            .background(Color(nsColor: .textBackgroundColor), in: RoundedRectangle(cornerRadius: 8))
            .overlay(
                RoundedRectangle(cornerRadius: 8)
                    .stroke(Color(nsColor: .separatorColor).opacity(0.35))
            )
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

    private func selectRepo(_ repo: OverviewRepoBucket?) {
        Task {
            do {
                try await model.selectRepo(repo)
            } catch {
                model.record(error: error)
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

private struct RepoEmptyRow: View {
    var body: some View {
        Label("No repo data for this range", systemImage: "tray")
            .foregroundStyle(.secondary)
            .frame(maxWidth: .infinity, alignment: .leading)
            .padding(12)
    }
}

private struct RepoHeaderRow: View {
    var body: some View {
        HStack(spacing: 12) {
            Text("Repo")
                .frame(maxWidth: .infinity, alignment: .leading)
            Text("Tokens")
                .frame(width: 110, alignment: .trailing)
            Text("Cost")
                .frame(width: 110, alignment: .trailing)
            Text("Tool calls")
                .frame(width: 110, alignment: .trailing)
        }
        .font(.caption.weight(.medium))
        .foregroundStyle(.secondary)
        .padding(.horizontal, 12)
        .padding(.vertical, 8)
    }
}

private struct RepoSummaryRow: View {
    let summary: OverviewRepoSummary
    let isSelected: Bool
    let costFormatter: (UInt64) -> String
    let action: () -> Void

    var body: some View {
        Button(action: action) {
            HStack(spacing: 12) {
                VStack(alignment: .leading, spacing: 3) {
                    HStack(spacing: 6) {
                        Image(systemName: isSelected ? "line.3.horizontal.decrease.circle.fill" : "folder")
                            .foregroundStyle(isSelected ? Color.accentColor : Color.secondary)
                        Text(summary.repo.displayName)
                            .font(.body.weight(.medium))
                            .lineLimit(1)
                    }
                    if let subtitle = summary.repo.subtitle {
                        Text(subtitle)
                            .font(.caption)
                            .foregroundStyle(.secondary)
                            .lineLimit(1)
                    }
                }
                .frame(maxWidth: .infinity, alignment: .leading)

                Text(summary.totals.totalTokens.formatted())
                    .monospacedDigit()
                    .frame(width: 110, alignment: .trailing)
                Text(costFormatter(summary.totals.costUsdNanos))
                    .monospacedDigit()
                    .frame(width: 110, alignment: .trailing)
                Text(summary.totals.toolCalls.formatted())
                    .monospacedDigit()
                    .frame(width: 110, alignment: .trailing)
            }
            .padding(.horizontal, 12)
            .padding(.vertical, 10)
            .contentShape(Rectangle())
            .background(isSelected ? Color.accentColor.opacity(0.12) : Color.clear)
        }
        .buttonStyle(.plain)
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

private extension OverviewRepoBucket {
    var subtitle: String? {
        switch self {
        case .noRepo:
            return "No repo attribute"
        case .repo(let identity):
            return identity.path?.rawValue
        }
    }
}
