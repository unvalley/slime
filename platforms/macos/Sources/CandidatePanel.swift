import AppKit

struct CandidatePanelItem: Equatable {
    let value: String
    let annotation: String?
}

func candidateAnnotationText(_ detail: RustEngine.CandidateDetail) -> String? {
    guard detail.annotation == UInt32(SLIME_CANDIDATE_ANNOTATION_CORRECTION.rawValue) else {
        return nil
    }
    return detail.detail.map { "\($0)に訂正" } ?? "訂正"
}

func candidatePanelFrame(
    anchor: NSRect,
    preferredWidth: CGFloat,
    visibleCount: Int,
    visibleFrame: NSRect
) -> NSRect {
    let gap: CGFloat = 4
    let width = min(preferredWidth, visibleFrame.width)
    let height = CGFloat(visibleCount) * CandidatePanel.rowHeight
    let belowY = anchor.minY - height - gap
    let aboveY = anchor.maxY + gap

    let y: CGFloat
    if belowY >= visibleFrame.minY {
        y = belowY
    } else if aboveY + height <= visibleFrame.maxY {
        y = aboveY
    } else {
        y = min(max(belowY, visibleFrame.minY), visibleFrame.maxY - height)
    }

    let x = min(max(anchor.minX, visibleFrame.minX), visibleFrame.maxX - width)
    return NSRect(x: x, y: y, width: width, height: height)
}

final class CandidatePanel {
    fileprivate static let rowHeight: CGFloat = 28
    private static let pageSize = 9

    var onCandidateClicked: ((Int) -> Void)? {
        get { candidateView.onCandidateClicked }
        set { candidateView.onCandidateClicked = newValue }
    }

    private let panel: NSPanel
    private let candidateView = CandidateListView(
        rowHeight: CandidatePanel.rowHeight,
        pageSize: CandidatePanel.pageSize
    )

    init() {
        panel = NSPanel(
            contentRect: .zero,
            styleMask: [.borderless, .nonactivatingPanel],
            backing: .buffered,
            defer: false
        )
        panel.level = .popUpMenu
        panel.hasShadow = true
        panel.isOpaque = false
        panel.backgroundColor = .clear
        panel.collectionBehavior = [.canJoinAllSpaces, .fullScreenAuxiliary]
        panel.ignoresMouseEvents = false
        panel.contentView = candidateView
    }

    func show(candidates: [CandidatePanelItem], selected: Int, anchor: NSRect) {
        candidateView.update(candidates: candidates, selected: selected)

        if panel.isVisible {
            panel.contentView?.needsDisplay = true
            return
        }

        let visibleCount = min(candidates.count, Self.pageSize)
        let anchorPoint = NSPoint(x: anchor.midX, y: anchor.midY)
        let screen = NSScreen.screens.first(where: { $0.frame.contains(anchorPoint) }) ?? NSScreen.main
        let visibleFrame = screen?.visibleFrame ?? NSRect(x: 0, y: 0, width: 112, height: 252)
        let frame = candidatePanelFrame(
            anchor: anchor,
            preferredWidth: candidateView.preferredWidth,
            visibleCount: visibleCount,
            visibleFrame: visibleFrame
        )

        panel.setFrame(frame, display: true)
        panel.orderFrontRegardless()
    }

    func hide() {
        panel.orderOut(nil)
    }
}

private final class CandidateListView: NSView {
    var onCandidateClicked: ((Int) -> Void)?

    private let rowHeight: CGFloat
    private let pageSize: Int
    private var candidates: [CandidatePanelItem] = []
    private var selected = 0
    private var pageStart = 0

    init(rowHeight: CGFloat, pageSize: Int) {
        self.rowHeight = rowHeight
        self.pageSize = pageSize
        super.init(frame: .zero)
        wantsLayer = true
        layer?.cornerRadius = 8
        layer?.masksToBounds = true
    }

    @available(*, unavailable)
    required init?(coder: NSCoder) {
        fatalError("init(coder:) has not been implemented")
    }

    override var isFlipped: Bool { true }

    var preferredWidth: CGFloat {
        let valueAttributes: [NSAttributedString.Key: Any] = [.font: NSFont.systemFont(ofSize: 15)]
        let annotationAttributes: [NSAttributedString.Key: Any] = [
            .font: NSFont.systemFont(ofSize: 10),
        ]
        let textWidth = candidates
            .map { candidate in
                let valueWidth = (candidate.value as NSString)
                    .size(withAttributes: valueAttributes).width
                let annotationWidth = candidate.annotation.map {
                    ($0 as NSString).size(withAttributes: annotationAttributes).width + 12
                } ?? 0
                return valueWidth + annotationWidth
            }
            .max() ?? 0
        return max(112, ceil(textWidth) + 54)
    }

    func update(candidates: [CandidatePanelItem], selected: Int) {
        self.candidates = candidates
        self.selected = candidates.indices.contains(selected) ? selected : 0
        pageStart = (self.selected / pageSize) * pageSize
        needsDisplay = true
    }

    override func draw(_ dirtyRect: NSRect) {
        NSColor.windowBackgroundColor.setFill()
        bounds.fill()

        let pageEnd = min(pageStart + pageSize, candidates.count)
        for (visibleRow, index) in (pageStart ..< pageEnd).enumerated() {
            let rowRect = NSRect(
                x: 4,
                y: CGFloat(visibleRow) * rowHeight + 3,
                width: bounds.width - 8,
                height: rowHeight - 6
            )
            let isSelected = index == selected
            if isSelected {
                NSColor.controlAccentColor.setFill()
                NSBezierPath(roundedRect: rowRect, xRadius: 6, yRadius: 6).fill()
            }

            let numberAttributes: [NSAttributedString.Key: Any] = [
                .font: NSFont.monospacedDigitSystemFont(ofSize: 10, weight: .regular),
                .foregroundColor: isSelected ? NSColor.white : NSColor.secondaryLabelColor,
            ]
            let paragraph = NSMutableParagraphStyle()
            paragraph.lineBreakMode = .byTruncatingTail
            let candidateAttributes: [NSAttributedString.Key: Any] = [
                .font: NSFont.systemFont(ofSize: 15),
                .foregroundColor: isSelected ? NSColor.white : NSColor.labelColor,
                .paragraphStyle: paragraph,
            ]
            let annotationAttributes: [NSAttributedString.Key: Any] = [
                .font: NSFont.systemFont(ofSize: 10),
                .foregroundColor: isSelected
                    ? NSColor.white.withAlphaComponent(0.82)
                    : NSColor.secondaryLabelColor,
            ]

            String(visibleRow + 1).draw(
                at: NSPoint(x: 10, y: rowRect.minY + 6),
                withAttributes: numberAttributes
            )
            var valueMaxX = rowRect.maxX - 8
            if let annotation = candidates[index].annotation {
                let annotationSize = (annotation as NSString)
                    .size(withAttributes: annotationAttributes)
                let annotationX = rowRect.maxX - annotationSize.width - 8
                annotation.draw(
                    at: NSPoint(x: annotationX, y: rowRect.minY + 7),
                    withAttributes: annotationAttributes
                )
                valueMaxX = annotationX - 8
            }
            candidates[index].value.draw(
                in: NSRect(
                    x: 30,
                    y: rowRect.minY + 3,
                    width: max(0, valueMaxX - 30),
                    height: rowRect.height
                ),
                withAttributes: candidateAttributes
            )
        }
    }

    override func mouseDown(with event: NSEvent) {
        let point = convert(event.locationInWindow, from: nil)
        let visibleRow = Int(point.y / rowHeight)
        let index = pageStart + visibleRow
        guard candidates.indices.contains(index) else { return }
        onCandidateClicked?(index)
    }
}
