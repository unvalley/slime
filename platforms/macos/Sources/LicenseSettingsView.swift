import SwiftUI

struct LicenseSettingsView: View {
    @ObservedObject var access = SlimeAccessController.shared
    @State private var licenseKey = ""

    var body: some View {
        Form {
            Section {
                HStack(alignment: .top, spacing: 14) {
                    Image(systemName: statusIcon)
                        .font(.title2)
                        .foregroundStyle(statusColor)
                        .frame(width: 28)
                    VStack(alignment: .leading, spacing: 5) {
                        Text(statusTitle)
                            .font(.headline)
                        Text(statusDescription)
                            .font(.callout)
                            .foregroundStyle(.secondary)
                    }
                }
                .padding(.vertical, 4)
            }

            if case .localDevelopment = access.status {
                Section("開発ビルド") {
                    Text("ローカル開発用ビルドでは課金確認を行いません。本番ビルドではPolarの設定が必須です。")
                        .foregroundStyle(.secondary)
                }
            } else {
                Section("Slimeをはじめる") {
                    VStack(alignment: .leading, spacing: 5) {
                        Text("14日間無料、その後は月額200円")
                            .font(.headline)
                        Text("無料期間中に解約した場合、料金はかかりません。支払いと解約はPolarで安全に管理されます。")
                            .font(.caption)
                            .foregroundStyle(.secondary)
                    }
                    Button("14日間無料で試す") {
                        access.openCheckout()
                    }
                    .buttonStyle(.borderedProminent)
                    .disabled(access.checkoutURL == nil)
                }

                Section("ライセンスキー") {
                    LabeledContent("キー") {
                        TextField(
                            "",
                            text: $licenseKey,
                            prompt: Text("XXXX-XXXX-XXXX-XXXX")
                        )
                        .labelsHidden()
                        .textFieldStyle(.roundedBorder)
                        .disabled(access.isValidating)
                    }
                    HStack {
                        Button(access.isValidating ? "確認中…" : "ライセンスを有効にする") {
                            Task {
                                if await access.activate(licenseKey) {
                                    licenseKey = ""
                                }
                            }
                        }
                        .disabled(access.isValidating || licenseKey.trimmingCharacters(
                            in: .whitespacesAndNewlines
                        ).isEmpty)
                        Spacer()
                        if access.status.allowsInput,
                           access.status != .localDevelopment
                        {
                            Button("このMacから削除", role: .destructive) {
                                access.removeLicense()
                            }
                        }
                    }
                    Text("Polarの購入完了画面またはメールに表示されたキーを入力してください。")
                        .font(.caption)
                        .foregroundStyle(.secondary)
                }
            }

            if let message = access.message {
                Section {
                    Text(message)
                        .font(.caption)
                        .foregroundStyle(messageColor)
                }
            }
        }
        .formStyle(.grouped)
        .task {
            await access.refreshStoredLicense()
        }
    }

    private var statusTitle: String {
        switch access.status {
        case .localDevelopment: "ローカル開発"
        case .needsLicense: "ライセンスが必要です"
        case .validating: "ライセンスを確認しています"
        case .licensed: "Slimeを利用できます"
        case .offlineGrace: "オフラインで利用中"
        case .connectionRequired: "インターネット接続が必要です"
        case .configurationError: "課金設定を確認してください"
        }
    }

    private var statusDescription: String {
        switch access.status {
        case .localDevelopment:
            "入力機能は制限されていません。"
        case .needsLicense:
            "無料トライアルを開始するか、発行済みのキーを入力してください。"
        case let .validating(allowsInput):
            allowsInput ? "確認中も入力を続けられます。" : "確認が終わるまで少しお待ちください。"
        case .licensed:
            "ライセンスは有効です。"
        case let .offlineGrace(until):
            "一時的にPolarへ接続できません。\(until.formatted(date: .abbreviated, time: .omitted))まで入力できます。"
        case .connectionRequired:
            "契約状態を確認するため、ネットワークに接続してもう一度お試しください。"
        case let .configurationError(message):
            message
        }
    }

    private var statusIcon: String {
        switch access.status {
        case .localDevelopment: "hammer"
        case .licensed: "checkmark.circle.fill"
        case .offlineGrace: "wifi.slash"
        case .validating: "arrow.triangle.2.circlepath"
        case .needsLicense: "key"
        case .connectionRequired, .configurationError: "exclamationmark.triangle.fill"
        }
    }

    private var statusColor: Color {
        switch access.status {
        case .licensed, .localDevelopment: .green
        case .offlineGrace, .validating: .secondary
        case .needsLicense: .accentColor
        case .connectionRequired, .configurationError: .orange
        }
    }

    private var messageColor: Color {
        if case .licensed = access.status { return .green }
        return .secondary
    }
}
