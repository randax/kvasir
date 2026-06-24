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
            let costPresentation = Self.costDashboardPresentation(for: snapshot)
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
                    totals(
                        snapshot.totals,
                        costDisplay: costPresentation.total,
                        showsToolCalls: snapshot.selectedModel == nil
                    )
                    modelDashboard(snapshot.modelBreakdown)
                    repoDashboard(snapshot.repoBreakdown, showsToolCalls: snapshot.selectedModel == nil)
                    charts(
                        snapshot.series,
                        costPresentation: costPresentation,
                        showsToolCalls: snapshot.selectedModel == nil
                    )
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

    nonisolated static func costDashboardPresentation(for snapshot: OverviewSnapshot) -> OverviewCostDashboardPresentation {
        snapshot.costDashboardPresentation
    }

    private func totals(
        _ totals: OverviewTotals,
        costDisplay: OverviewCostDisplay,
        showsToolCalls: Bool
    ) -> some View {
        HStack(spacing: 12) {
            TotalTile(title: "Tokens", value: totals.totalTokens.formatted(), systemImage: "sum")
            TotalTile(
                title: "Cost",
                value: usd(totals.costUsdNanos),
                systemImage: "dollarsign",
                detail: costDisplay.estimateLabel
            )
            if showsToolCalls {
                TotalTile(title: "Tool calls", value: totals.toolCalls.formatted(), systemImage: "hammer")
            }
        }
    }

    private func repoDashboard(_ repos: [OverviewRepoSummary], showsToolCalls: Bool) -> some View {
        VStack(alignment: .leading, spacing: 10) {
            HStack(spacing: 12) {
                Text("Repos")
                    .font(.headline)

                if let selectedRepo = model.selectedRepo {
                    Text(selectedRepo.displayName)
                        .font(.caption.weight(.medium))
                    .foregroundStyle(.secondary)
                    .lineLimit(1)
                    .truncationMode(.middle)
                    .help(selectedRepo.displayName)
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
                    RepoHeaderRow(showsToolCalls: showsToolCalls)
                    ForEach(repos, id: \.repo) { summary in
                        RepoSummaryRow(
                            summary: summary,
                            isSelected: model.selectedRepo == summary.repo,
                            showsToolCalls: showsToolCalls,
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

    private func modelDashboard(_ models: [OverviewModelSummary]) -> some View {
        VStack(alignment: .leading, spacing: 10) {
            HStack(spacing: 12) {
                Text("Models")
                    .font(.headline)

                if let selectedModel = model.selectedModel {
                    Text(selectedModel.displayName())
                        .font(.caption.weight(.medium))
                        .foregroundStyle(.secondary)
                        .lineLimit(1)
                        .truncationMode(.middle)
                        .help(selectedModel.displayName())
                }

                Spacer()

                Button {
                    selectModel(nil)
                } label: {
                    Label("All models", systemImage: "cpu")
                }
                .disabled(model.selectedModel == nil)
            }

            VStack(spacing: 0) {
                if models.isEmpty {
                    ModelEmptyRow()
                } else {
                    ModelHeaderRow()
                    ForEach(models, id: \.model) { summary in
                        ModelSummaryRow(
                            summary: summary,
                            isSelected: model.selectedModel == summary.model,
                            costFormatter: usd
                        ) {
                            selectModel(summary.model)
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

    private func charts(
        _ series: [OverviewSeriesPoint],
        costPresentation: OverviewCostDashboardPresentation,
        showsToolCalls: Bool
    ) -> some View {
        Grid(horizontalSpacing: 16, verticalSpacing: 16) {
            GridRow {
                MetricChart(title: "Tokens", series: series, color: .teal) { $0.totalTokens }
                MetricChart(
                    title: "Cost",
                    detail: costPresentation.total.estimateLabel,
                    series: series,
                    color: .indigo,
                    costDisplay: { point in
                        guard let index = series.firstIndex(where: { $0.day == point.day }),
                              costPresentation.series.indices.contains(index) else {
                            return point.costDisplay
                        }
                        return costPresentation.series[index]
                    }
                ) { $0.costUsdNanos }
            }
            if showsToolCalls {
                GridRow {
                    MetricChart(title: "Tool calls", series: series, color: .orange) { $0.toolCalls }
                        .gridCellColumns(2)
                }
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

    private func selectModel(_ selectedModel: OverviewModelName?) {
        Task {
            do {
                try await model.selectModel(selectedModel)
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

private struct ModelEmptyRow: View {
    var body: some View {
        Label("No model data for this range", systemImage: "tray")
            .foregroundStyle(.secondary)
            .frame(maxWidth: .infinity, alignment: .leading)
            .padding(12)
    }
}

private struct RepoHeaderRow: View {
    let showsToolCalls: Bool

    var body: some View {
        HStack(spacing: 12) {
            Text("Repo")
                .frame(maxWidth: .infinity, alignment: .leading)
            Text("Tokens")
                .frame(width: 110, alignment: .trailing)
            Text("Cost")
                .frame(width: 132, alignment: .trailing)
            if showsToolCalls {
                Text("Tool calls")
                    .frame(width: 110, alignment: .trailing)
            }
        }
        .font(.caption.weight(.medium))
        .foregroundStyle(.secondary)
        .padding(.horizontal, 12)
        .padding(.vertical, 8)
    }
}

private struct ModelHeaderRow: View {
    var body: some View {
        HStack(spacing: 12) {
            Text("Model")
                .frame(maxWidth: .infinity, alignment: .leading)
            Text("Tokens")
                .frame(width: 110, alignment: .trailing)
            Text("Cost")
                .frame(width: 132, alignment: .trailing)
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
    let showsToolCalls: Bool
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
                            .truncationMode(.middle)
                            .help(summary.repo.displayName)
                    }
                    if let subtitle = summary.repo.subtitle {
                        Text(subtitle)
                            .font(.caption)
                            .foregroundStyle(.secondary)
                            .lineLimit(1)
                            .truncationMode(.middle)
                            .help(subtitle)
                    }
                }
                .frame(maxWidth: .infinity, alignment: .leading)

                Text(summary.totals.totalTokens.formatted())
                    .monospacedDigit()
                    .frame(width: 110, alignment: .trailing)
                CostValue(
                    display: summary.totals.costDisplay,
                    formatter: costFormatter
                )
                if showsToolCalls {
                    Text(summary.totals.toolCalls.formatted())
                        .monospacedDigit()
                        .frame(width: 110, alignment: .trailing)
                }
            }
            .padding(.horizontal, 12)
            .padding(.vertical, 10)
            .contentShape(Rectangle())
            .background(isSelected ? Color.accentColor.opacity(0.12) : Color.clear)
        }
        .buttonStyle(.plain)
    }
}

private struct ModelSummaryRow: View {
    let summary: OverviewModelSummary
    let isSelected: Bool
    let costFormatter: (UInt64) -> String
    let action: () -> Void

    var body: some View {
        Button(action: action) {
            HStack(spacing: 12) {
                HStack(spacing: 6) {
                    Image(systemName: isSelected ? "line.3.horizontal.decrease.circle.fill" : "cpu")
                        .foregroundStyle(isSelected ? Color.accentColor : Color.secondary)
                    Text(summary.model.displayName())
                        .font(.body.weight(.medium))
                        .lineLimit(1)
                        .truncationMode(.middle)
                }
                .frame(maxWidth: .infinity, alignment: .leading)

                Text(summary.totals.totalTokens.formatted())
                    .monospacedDigit()
                    .frame(width: 110, alignment: .trailing)
                CostValue(
                    display: summary.totals.costDisplay,
                    formatter: costFormatter
                )
            }
            .padding(.horizontal, 12)
            .padding(.vertical, 10)
            .contentShape(Rectangle())
            .background(isSelected ? Color.accentColor.opacity(0.12) : Color.clear)
        }
        .buttonStyle(.plain)
        .help(summary.model.displayName())
    }
}

private struct TotalTile: View {
    let title: String
    let value: String
    let systemImage: String
    var detail: String? = nil

    var body: some View {
        VStack(alignment: .leading, spacing: 8) {
            Label(title, systemImage: systemImage)
                .font(.caption.weight(.medium))
                .foregroundStyle(.secondary)
            Text(value)
                .font(.system(.title2, design: .rounded, weight: .semibold))
                .monospacedDigit()
            if let detail {
                EstimateBadge(text: detail)
            }
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
    var detail: String? = nil
    let series: [OverviewSeriesPoint]
    let color: Color
    var costDisplay: (OverviewSeriesPoint) -> OverviewCostDisplay = {
        OverviewCostDisplay(costUsdNanos: $0.costUsdNanos, source: nil)
    }
    let value: (OverviewSeriesPoint) -> UInt64

    var body: some View {
        VStack(alignment: .leading, spacing: 12) {
            HStack(spacing: 8) {
                Text(title)
                    .font(.headline)
                if let detail {
                    EstimateBadge(text: detail)
                }
            }

            Chart(series, id: \.day) { point in
                let display = costDisplay(point)
                BarMark(
                    x: .value("Day", point.day.shortLabel),
                    y: .value(title, value(point))
                )
                .foregroundStyle((display.usesEstimatedCost ? Color.orange : color).gradient)
                .annotation(position: .top) {
                    if let marker = display.chartMarkerLabel {
                        Text(marker)
                            .font(.caption2.weight(.medium))
                            .foregroundStyle(.orange)
                            .lineLimit(1)
                            .minimumScaleFactor(0.8)
                            .accessibilityLabel(display.estimateLabel ?? marker)
                    }
                }
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

private struct CostValue: View {
    let display: OverviewCostDisplay
    let formatter: (UInt64) -> String

    var body: some View {
        VStack(alignment: .trailing, spacing: 3) {
            Text(formatter(display.costUsdNanos))
                .monospacedDigit()
            if let detail = display.estimateLabel {
                EstimateBadge(text: detail)
            }
        }
        .frame(width: 132, alignment: .trailing)
    }
}

private struct EstimateBadge: View {
    let text: String

    var body: some View {
        Label(text, systemImage: OverviewCostDisplay.estimateBadgeSystemImage)
            .labelStyle(.titleAndIcon)
            .font(.caption2.weight(.medium))
            .foregroundStyle(.orange)
            .lineLimit(1)
            .truncationMode(.tail)
            .minimumScaleFactor(0.8)
            .help(text)
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
