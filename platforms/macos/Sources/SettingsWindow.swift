import AppKit
import SwiftUI

@MainActor
final class SettingsModel: ObservableObject {
    @Published var dictionary: [UserDictionaryEntry] = []
    @Published var history: [InputHistoryEntry] = []
    @Published var reading = ""
    @Published var surface = ""
    @Published var dictionaryQuery = ""
    @Published var historyQuery = ""
    @Published var errorMessage: String?
    @Published var noticeMessage: String?

    private let store = UserDataStore.shared
    private var dictionaryBase: Data?
    private var historyBase: Data?

    init() {
        reloadDictionary()
        reloadHistory()
    }

    var liveConversion: Bool {
        get { IMEPreferences.liveConversion }
        set {
            objectWillChange.send()
            IMEPreferences.liveConversion = newValue
        }
    }

    var historyCompletion: Bool {
        get { IMEPreferences.historyCompletion }
        set {
            objectWillChange.send()
            IMEPreferences.historyCompletion = newValue
        }
    }

    var historyLearning: Bool {
        get { IMEPreferences.historyLearning }
        set {
            objectWillChange.send()
            IMEPreferences.historyLearning = newValue
        }
    }

    func isDictionaryPackEnabled(_ mask: UInt32) -> Bool {
        IMEPreferences.dictionaryPacks & mask != 0
    }

    func setDictionaryPack(_ mask: UInt32, enabled: Bool) {
        objectWillChange.send()
        if enabled {
            IMEPreferences.dictionaryPacks |= mask
        } else {
            IMEPreferences.dictionaryPacks &= ~mask
        }
    }

    var filteredDictionary: [UserDictionaryEntry] {
        let query = dictionaryQuery.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !query.isEmpty else { return dictionary }
        return dictionary.filter {
            $0.reading.localizedStandardContains(query)
                || $0.surface.localizedStandardContains(query)
        }
    }

    var filteredHistory: [InputHistoryEntry] {
        let query = historyQuery.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !query.isEmpty else { return history }
        return history.filter {
            $0.reading.localizedStandardContains(query)
                || $0.surface.localizedStandardContains(query)
        }
    }

    var lowValueHistoryCount: Int {
        history.lazy.filter { !$0.isUsefulForCompletion }.count
    }

    func addDictionaryEntry() {
        let normalizedReading = normalizedDictionaryReading(reading)
        let normalizedSurface = surface.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !normalizedReading.isEmpty, !normalizedSurface.isEmpty else {
            errorMessage = UserDataStoreError.invalidEntry.localizedDescription
            return
        }
        guard !dictionary.contains(where: {
            $0.reading == normalizedReading && $0.surface == normalizedSurface
        }) else {
            reading = ""
            surface = ""
            return
        }

        dictionary.append(UserDictionaryEntry(
            reading: normalizedReading,
            surface: normalizedSurface
        ))
        if persistDictionary() {
            reading = ""
            surface = ""
        }
    }

    func removeDictionaryEntry(_ entry: UserDictionaryEntry) {
        let previous = dictionary
        dictionary.removeAll { $0.id == entry.id }
        if !persistDictionary() {
            dictionary = previous
        }
    }

    func reloadDictionary() {
        do {
            let snapshot = try store.loadDictionary()
            dictionary = snapshot.entries
            dictionaryBase = snapshot.base
            errorMessage = nil
            noticeMessage = nil
            NotificationCenter.default.post(name: .unvalleyUserDataDidChange, object: nil)
        } catch {
            errorMessage = error.localizedDescription
        }
    }

    func reloadHistory() {
        do {
            let snapshot = try store.loadHistorySnapshot()
            history = snapshot.entries
            historyBase = snapshot.base
            errorMessage = nil
            noticeMessage = nil
        } catch {
            errorMessage = error.localizedDescription
        }
    }

    func clearHistory() {
        do {
            historyBase = try store.clearHistory(replacing: historyBase)
            history = []
            errorMessage = nil
            noticeMessage = "入力履歴を消去しました。"
        } catch {
            errorMessage = error.localizedDescription
        }
    }

    func removeHistoryEntry(_ entry: InputHistoryEntry) {
        do {
            historyBase = try store.removeHistoryEntry(
                entry,
                from: history,
                replacing: historyBase
            )
            history.removeAll { $0.id == entry.id }
            errorMessage = nil
            noticeMessage = "「\(entry.surface)」を履歴から削除しました。"
        } catch {
            errorMessage = error.localizedDescription
        }
    }

    func compactHistory() {
        let removedCount = lowValueHistoryCount
        guard removedCount > 0 else { return }
        do {
            historyBase = try store.compactHistory(history, replacing: historyBase)
            history.removeAll { !$0.isUsefulForCompletion }
            errorMessage = nil
            noticeMessage = "補完に使われない履歴を\(removedCount)件削除しました。"
        } catch {
            errorMessage = error.localizedDescription
        }
    }

    func importDictionary() {
        let panel = NSOpenPanel()
        panel.title = "辞書を読み込む"
        panel.message = "Google日本語入力、Microsoft IME、またはMacのユーザ辞書を選択してください。"
        panel.allowsMultipleSelection = false
        panel.canChooseDirectories = false
        guard panel.runModal() == .OK, let url = panel.url else { return }

        do {
            let data = try Data(contentsOf: url)
            let result = try DictionaryImporter.parse(
                data: data,
                fileExtension: url.pathExtension
            )
            let existing = Set(dictionary.map { "\($0.reading)\u{0}\($0.surface)" })
            let additions = result.entries.filter {
                !existing.contains("\($0.reading)\u{0}\($0.surface)")
            }
            dictionary.append(contentsOf: additions)
            guard persistDictionary() else {
                dictionary.removeLast(additions.count)
                return
            }
            let ignored = result.skippedCount + result.entries.count - additions.count
            noticeMessage = "\(result.formatName)から\(additions.count)件を追加しました"
                + (ignored > 0 ? "（重複・無効\(ignored)件を除外）。" : "。")
        } catch {
            errorMessage = error.localizedDescription
        }
    }

    func exportDictionary() {
        let panel = NSSavePanel()
        panel.title = "ユーザー辞書を書き出す"
        panel.nameFieldStringValue = "Unvalley User Dictionary.tsv"
        guard panel.runModal() == .OK, let url = panel.url else { return }

        do {
            try store.dictionaryData(dictionary).write(to: url, options: .atomic)
            errorMessage = nil
            noticeMessage = "\(dictionary.count)件を「\(url.lastPathComponent)」へ書き出しました。"
        } catch {
            errorMessage = error.localizedDescription
        }
    }

    private func persistDictionary() -> Bool {
        do {
            dictionaryBase = try store.saveDictionary(dictionary, replacing: dictionaryBase)
            errorMessage = nil
            noticeMessage = nil
            return true
        } catch {
            errorMessage = error.localizedDescription
            return false
        }
    }
}

/// Loads the bundled words of the domain dictionaries selected by `mask`.
/// The default returns nothing so this file can build without the Rust
/// engine (settings preview); the app installs the FFI-backed loader at
/// launch.
enum DomainDictionaryCatalog {
    static var loader: (UInt32) throws -> [DomainDictionaryWord] = { _ in [] }
}

enum SettingsTab: String {
    case general
    case dictionary
    case history
}

struct SettingsRootView: View {
    @StateObject private var model = SettingsModel()
    @State private var selection: SettingsTab

    init(initialTab: SettingsTab = .general) {
        _selection = State(initialValue: initialTab)
    }

    var body: some View {
        TabView(selection: $selection) {
            GeneralSettingsView(model: model)
                .tabItem { Label("一般", systemImage: "gearshape") }
                .tag(SettingsTab.general)
            DictionarySettingsView(model: model)
                .tabItem { Label("ユーザー辞書", systemImage: "character.book.closed") }
                .tag(SettingsTab.dictionary)
            HistorySettingsView(model: model)
                .tabItem { Label("入力履歴", systemImage: "clock.arrow.circlepath") }
                .tag(SettingsTab.history)
        }
        .padding(24)
        .frame(minWidth: 680, minHeight: 520)
        .alert(
            "保存できませんでした",
            isPresented: Binding(
                get: { model.errorMessage != nil },
                set: { if !$0 { model.errorMessage = nil } }
            )
        ) {
            Button("OK", role: .cancel) {}
        } message: {
            Text(model.errorMessage ?? "")
        }
    }
}

private struct GeneralSettingsView: View {
    @ObservedObject var model: SettingsModel

    var body: some View {
        Form {
            Section("変換") {
                Toggle("ライブ変換", isOn: Binding(
                    get: { model.liveConversion },
                    set: { model.liveConversion = $0 }
                ))
                Text("入力中の読みを、Spaceを押さずに最良候補へ変換します。")
                    .font(.caption)
                    .foregroundStyle(.secondary)
            }
            Section("補完") {
                Toggle("入力履歴から候補を表示", isOn: Binding(
                    get: { model.historyCompletion },
                    set: { model.historyCompletion = $0 }
                ))
                Toggle("新しい確定結果を学習", isOn: Binding(
                    get: { model.historyLearning },
                    set: { model.historyLearning = $0 }
                ))
                Text("候補表示と新規学習は別々に停止できます。履歴は最大500件、このMac内だけに保存され、利用した補完候補は次回から優先されます。")
                    .font(.caption)
                    .foregroundStyle(.secondary)
            }
            Section("分野別辞書") {
                DomainDictionaryToggle(
                    title: "テクノロジー",
                    description: "プログラミング、インフラ、ソフトウェア開発の用語",
                    mask: 1 << 0,
                    model: model
                )
                DomainDictionaryToggle(
                    title: "ビジネス",
                    description: "契約、請求、会計、社内手続きの用語",
                    mask: 1 << 1,
                    model: model
                )
                DomainDictionaryToggle(
                    title: "クリエイティブ",
                    description: "デザイン、映像、制作進行の用語",
                    mask: 1 << 2,
                    model: model
                )
                Text("必要な辞書だけを有効にできます。基本辞書とユーザー辞書は常に利用されます。")
                    .font(.caption)
                    .foregroundStyle(.secondary)
            }
        }
        .formStyle(.grouped)
    }
}

private struct DomainDictionaryToggle: View {
    let title: String
    let description: String
    let mask: UInt32
    @ObservedObject var model: SettingsModel
    @State private var showsWords = false

    var body: some View {
        HStack(spacing: 12) {
            Toggle(isOn: Binding(
                get: { model.isDictionaryPackEnabled(mask) },
                set: { model.setDictionaryPack(mask, enabled: $0) }
            )) {
                VStack(alignment: .leading, spacing: 2) {
                    Text(title)
                    Text(description)
                        .font(.caption)
                        .foregroundStyle(.secondary)
                }
            }
            Button {
                showsWords = true
            } label: {
                Image(systemName: "list.bullet.rectangle")
            }
            .buttonStyle(.borderless)
            .help("収録語を見る")
        }
        .sheet(isPresented: $showsWords) {
            DomainDictionaryWordsView(title: title, mask: mask)
        }
    }
}

private struct DomainDictionaryWordsView: View {
    let title: String
    let mask: UInt32
    @Environment(\.dismiss) private var dismiss
    @State private var words: [DomainDictionaryWord] = []
    @State private var query = ""
    @State private var loadError: String?

    private var filteredWords: [DomainDictionaryWord] {
        let query = query.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !query.isEmpty else { return words }
        return words.filter {
            $0.reading.localizedStandardContains(query)
                || $0.surface.localizedStandardContains(query)
        }
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 12) {
            VStack(alignment: .leading, spacing: 4) {
                Text("「\(title)」の収録語")
                    .font(.title3.weight(.semibold))
                Text("この辞書はアプリに組み込まれていて、編集はユーザー辞書で行います。")
                    .font(.caption)
                    .foregroundStyle(.secondary)
            }

            HStack(spacing: 8) {
                Image(systemName: "magnifyingglass")
                    .foregroundStyle(.secondary)
                TextField("読みまたは単語を検索", text: $query)
                    .textFieldStyle(.plain)
            }
            .padding(.horizontal, 10)
            .padding(.vertical, 7)
            .background(.quaternary.opacity(0.7), in: RoundedRectangle(cornerRadius: 7))

            List(filteredWords) { word in
                HStack {
                    Text(word.reading)
                        .frame(width: 180, alignment: .leading)
                        .foregroundStyle(.secondary)
                    Text(word.surface)
                }
            }
            .overlay {
                if filteredWords.isEmpty {
                    EmptyStateView(
                        title: words.isEmpty ? "収録語を読み込めませんでした" : "一致する単語がありません",
                        systemImage: "character.book.closed",
                        description: words.isEmpty
                            ? (loadError ?? "IMEを再起動してからもう一度開いてください。")
                            : "別の読みまたは単語で検索してください。"
                    )
                }
            }

            HStack {
                Text("\(words.count)語")
                    .foregroundStyle(.secondary)
                Spacer()
                Button("閉じる") { dismiss() }
                    .keyboardShortcut(.cancelAction)
            }
        }
        .padding(20)
        .frame(width: 480, height: 440)
        .onAppear {
            do {
                words = try DomainDictionaryCatalog.loader(mask)
            } catch {
                loadError = error.localizedDescription
            }
        }
    }
}

private struct DictionarySettingsView: View {
    @ObservedObject var model: SettingsModel

    var body: some View {
        VStack(alignment: .leading, spacing: 12) {
            VStack(alignment: .leading, spacing: 4) {
                Text("ユーザー辞書")
                    .font(.title2.weight(.semibold))
                Text("単語を直接追加するか、ほかの日本語入力から移行できます。")
                    .foregroundStyle(.secondary)
            }

            HStack(spacing: 8) {
                TextField("読み（ひらがな）", text: $model.reading)
                TextField("単語", text: $model.surface)
                Button("追加") { model.addDictionaryEntry() }
                    .keyboardShortcut(.return, modifiers: [])
            }

            HStack(spacing: 8) {
                Image(systemName: "magnifyingglass")
                    .foregroundStyle(.secondary)
                TextField("読みまたは単語を検索", text: $model.dictionaryQuery)
                    .textFieldStyle(.plain)
            }
            .padding(.horizontal, 10)
            .padding(.vertical, 7)
            .background(.quaternary.opacity(0.7), in: RoundedRectangle(cornerRadius: 7))

            List(model.filteredDictionary) { entry in
                HStack {
                    Text(entry.reading)
                        .frame(width: 180, alignment: .leading)
                    Text(entry.surface)
                    Spacer()
                    Button {
                        model.removeDictionaryEntry(entry)
                    } label: {
                        Image(systemName: "trash")
                    }
                    .buttonStyle(.borderless)
                    .help("削除")
                }
            }
            .overlay {
                if model.filteredDictionary.isEmpty {
                    EmptyStateView(
                        title: model.dictionary.isEmpty ? "ユーザー辞書は空です" : "一致する単語がありません",
                        systemImage: "character.book.closed",
                        description: model.dictionary.isEmpty
                            ? "直接追加するか、既存の辞書を読み込めます。"
                            : "別の読みまたは単語で検索してください。"
                    )
                }
            }

            HStack {
                Button("辞書を読み込む…") { model.importDictionary() }
                Button("書き出す…") { model.exportDictionary() }
                    .disabled(model.dictionary.isEmpty)
                Menu("その他") {
                    Button("再読み込み") { model.reloadDictionary() }
                    Button("Finderで表示") { UserDataStore.shared.revealDictionary() }
                }
                Spacer()
                if let notice = model.noticeMessage {
                    Text(notice)
                        .lineLimit(1)
                        .help(notice)
                        .foregroundStyle(.secondary)
                } else {
                    Text("\(model.dictionary.count)件")
                        .foregroundStyle(.secondary)
                }
            }
        }
        .padding(.top, 8)
    }
}

private struct HistorySettingsView: View {
    @ObservedObject var model: SettingsModel
    @State private var confirmsClear = false

    var body: some View {
        VStack(alignment: .leading, spacing: 12) {
            VStack(alignment: .leading, spacing: 4) {
                Text("入力履歴")
                    .font(.title2.weight(.semibold))
                Text("補完と変換順位に使われる、このMac内だけの学習データです。")
                    .foregroundStyle(.secondary)
            }

            HStack(spacing: 8) {
                Image(systemName: "magnifyingglass")
                    .foregroundStyle(.secondary)
                TextField("読みまたは確定結果を検索", text: $model.historyQuery)
                    .textFieldStyle(.plain)
            }
            .padding(.horizontal, 10)
            .padding(.vertical, 7)
            .background(.quaternary.opacity(0.7), in: RoundedRectangle(cornerRadius: 7))

            List(model.filteredHistory) { entry in
                HStack {
                    VStack(alignment: .leading, spacing: 2) {
                        Text(entry.surface)
                        Text(entry.reading)
                            .font(.caption)
                            .foregroundStyle(.secondary)
                    }
                    Spacer()
                    Text("\(entry.count)回")
                        .foregroundStyle(.secondary)
                    Text(
                        entry.lastUsed,
                        format: .relative(presentation: .named, unitsStyle: .abbreviated)
                    )
                        .environment(\.locale, Locale(identifier: "ja_JP"))
                        .lineLimit(1)
                        .frame(width: 100, alignment: .trailing)
                        .foregroundStyle(.secondary)
                    Button {
                        model.removeHistoryEntry(entry)
                    } label: {
                        Image(systemName: "trash")
                    }
                    .buttonStyle(.borderless)
                    .help("この履歴を削除")
                }
            }
            .overlay {
                if model.filteredHistory.isEmpty {
                    EmptyStateView(
                        title: model.history.isEmpty ? "入力履歴はありません" : "一致する履歴がありません",
                        systemImage: "clock.arrow.circlepath",
                        description: model.history.isEmpty
                            ? "履歴補完を有効にすると、役立つ確定結果だけが表示されます。"
                            : "別の読みまたは確定結果で検索してください。"
                    )
                }
            }

            HStack {
                Button("再読み込み") { model.reloadHistory() }
                Button("使われない履歴を整理（\(model.lowValueHistoryCount)件）") {
                    model.compactHistory()
                }
                    .disabled(model.lowValueHistoryCount == 0)
                    .help("現在の学習条件では補完に使われない履歴だけを削除")
                Spacer()
                if let notice = model.noticeMessage {
                    Text(notice)
                        .lineLimit(1)
                        .help(notice)
                        .foregroundStyle(.secondary)
                } else {
                    Text("\(model.history.count)件")
                        .foregroundStyle(.secondary)
                }
                Button("すべて消去…", role: .destructive) { confirmsClear = true }
                    .disabled(model.history.isEmpty)
            }
        }
        .padding(.top, 8)
        .confirmationDialog(
            "入力履歴をすべて消去しますか？",
            isPresented: $confirmsClear
        ) {
            Button("すべて消去", role: .destructive) { model.clearHistory() }
            Button("キャンセル", role: .cancel) {}
        } message: {
            Text("この操作は取り消せません。ユーザー辞書は削除されません。")
        }
    }
}

private struct EmptyStateView: View {
    let title: String
    let systemImage: String
    let description: String

    var body: some View {
        VStack(spacing: 8) {
            Image(systemName: systemImage)
                .font(.system(size: 28))
                .foregroundStyle(.secondary)
            Text(title)
                .font(.headline)
            Text(description)
                .font(.caption)
                .foregroundStyle(.secondary)
        }
        .multilineTextAlignment(.center)
    }
}

@MainActor
final class SettingsWindowController: NSWindowController, NSWindowDelegate {
    static let shared = SettingsWindowController()

    private init() {
        let hostingController = NSHostingController(rootView: SettingsRootView())
        let window = NSWindow(contentViewController: hostingController)
        window.title = "Unvalley IME設定"
        window.styleMask = [.titled, .closable, .miniaturizable, .resizable]
        window.isReleasedWhenClosed = false
        window.contentMinSize = NSSize(width: 680, height: 520)
        window.setContentSize(NSSize(width: 760, height: 600))
        super.init(window: window)
        window.delegate = self
    }

    @available(*, unavailable)
    required init?(coder: NSCoder) {
        fatalError("init(coder:) has not been implemented")
    }

    func present(initialTab: SettingsTab = .general) {
        NSApp.setActivationPolicy(.accessory)
        window?.contentViewController = NSHostingController(
            rootView: SettingsRootView(initialTab: initialTab)
        )
        showWindow(nil)
        window?.center()
        window?.makeKeyAndOrderFront(nil)
        NSApp.activate(ignoringOtherApps: true)
    }
}

@MainActor
final class SettingsStatusItem: NSObject, NSMenuDelegate {
    static let shared = SettingsStatusItem()

    private let statusItem = NSStatusBar.system.statusItem(withLength: NSStatusItem.squareLength)
    private let liveConversionItem = NSMenuItem(
        title: "ライブ変換",
        action: #selector(toggleLiveConversion(_:)),
        keyEquivalent: ""
    )
    private let historyCompletionItem = NSMenuItem(
        title: "履歴から補完",
        action: #selector(toggleHistoryCompletion(_:)),
        keyEquivalent: ""
    )
    private let historyLearningItem = NSMenuItem(
        title: "入力結果を学習",
        action: #selector(toggleHistoryLearning(_:)),
        keyEquivalent: ""
    )

    private override init() {
        super.init()
        if let button = statusItem.button {
            button.image = NSImage(systemSymbolName: "gearshape", accessibilityDescription: "Unvalley IME設定")
            button.toolTip = "Unvalley IME設定"
        }

        let menu = NSMenu(title: "Unvalley IME")
        menu.delegate = self
        liveConversionItem.target = self
        historyCompletionItem.target = self
        historyLearningItem.target = self
        menu.addItem(liveConversionItem)
        menu.addItem(historyCompletionItem)
        menu.addItem(historyLearningItem)
        menu.addItem(.separator())
        let settingsItem = NSMenuItem(
            title: "Unvalley IME設定…",
            action: #selector(openSettings(_:)),
            keyEquivalent: ","
        )
        settingsItem.target = self
        menu.addItem(settingsItem)
        statusItem.menu = menu
    }

    func install() {
        updateStates()
    }

    func menuWillOpen(_ menu: NSMenu) {
        updateStates()
    }

    @objc private func toggleLiveConversion(_ sender: Any?) {
        IMEPreferences.liveConversion.toggle()
        updateStates()
    }

    @objc private func toggleHistoryCompletion(_ sender: Any?) {
        IMEPreferences.historyCompletion.toggle()
        updateStates()
    }

    @objc private func toggleHistoryLearning(_ sender: Any?) {
        IMEPreferences.historyLearning.toggle()
        updateStates()
    }

    @objc private func openSettings(_ sender: Any?) {
        SettingsWindowController.shared.present()
    }

    private func updateStates() {
        liveConversionItem.state = IMEPreferences.liveConversion ? .on : .off
        historyCompletionItem.state = IMEPreferences.historyCompletion ? .on : .off
        historyLearningItem.state = IMEPreferences.historyLearning ? .on : .off
    }
}
