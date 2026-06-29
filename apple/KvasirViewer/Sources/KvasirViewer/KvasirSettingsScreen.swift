import SwiftUI
import KvasirViewerCore

struct KvasirSettingsScreen: View {
    @ObservedObject var model: KvasirViewerModel
    @State private var isConfirmingClearAllData = false
    @State private var confirmationText = ""
    @State private var isClearingAllData = false
    @State private var clearAllDataErrorMessage: String?
    @State private var clearAllDataStatusMessage: String?

    var body: some View {
        Form {
            Section("Data") {
                VStack(alignment: .leading, spacing: 10) {
                    Button(role: .destructive) {
                        clearAllDataErrorMessage = nil
                        clearAllDataStatusMessage = nil
                        confirmationText = ""
                        isConfirmingClearAllData = true
                    } label: {
                        Label("Clear All Data", systemImage: "trash")
                    }
                    .disabled(!model.canClearAllData)

                    if let clearAllDataStatusMessage {
                        Text(clearAllDataStatusMessage)
                            .font(.caption)
                            .foregroundStyle(.secondary)
                    }

                    if let clearAllDataErrorMessage {
                        Text(clearAllDataErrorMessage)
                            .font(.caption)
                            .foregroundStyle(.red)
                    }
                }
            }
        }
        .formStyle(.grouped)
        .frame(width: 420)
        .sheet(isPresented: $isConfirmingClearAllData) {
            clearAllDataConfirmation
        }
    }

    private var clearAllDataConfirmation: some View {
        VStack(alignment: .leading, spacing: 16) {
            Text("Clear All Data")
                .font(.headline)

            Text("This deletes all gathered telemetry data from the local database and cannot be undone.")
                .foregroundStyle(.secondary)

            TextField("DELETE ALL DATA", text: $confirmationText)
                .textFieldStyle(.roundedBorder)
                .disabled(isClearingAllData)

            if let clearAllDataErrorMessage {
                Text(clearAllDataErrorMessage)
                    .font(.caption)
                    .foregroundStyle(.red)
            }

            HStack {
                Spacer()
                Button("Cancel") {
                    isConfirmingClearAllData = false
                }
                .disabled(isClearingAllData)

                Button(role: .destructive) {
                    Task {
                        await clearAllData()
                    }
                } label: {
                    if isClearingAllData {
                        ProgressView()
                            .controlSize(.small)
                    } else {
                        Label("Clear", systemImage: "trash")
                    }
                }
                .disabled(confirmationText != "DELETE ALL DATA" || isClearingAllData)
            }
        }
        .padding(24)
        .frame(width: 420)
    }

    @MainActor
    private func clearAllData() async {
        isClearingAllData = true
        clearAllDataErrorMessage = nil
        defer {
            isClearingAllData = false
        }

        do {
            let outcome = try await model.clearAllData()
            confirmationText = ""
            clearAllDataStatusMessage = "All data cleared"
            isConfirmingClearAllData = false
            if case .refreshFailed(let message) = outcome {
                clearAllDataErrorMessage = "Data was cleared, but the overview could not be refreshed: \(message)"
            }
        } catch {
            clearAllDataErrorMessage = error.localizedDescription
            model.record(error: error)
        }
    }
}
