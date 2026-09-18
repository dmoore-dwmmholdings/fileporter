import PhotosUI
import SwiftUI
import UniformTypeIdentifiers

/// 01 · Transport. Pick pads, pick something to send, watch it go.
struct TransportScreen: View {
    @Environment(AppModel.self) private var model
    @Environment(ArrivalPresenter.self) private var presenter

    @State private var photoSelection: [PhotosPickerItem] = []
    @State private var showingPhotos = false
    @State private var importing: ImportKind?
    @State private var landed = false
    @State private var completedCount = 0
    @State private var preparing = false

    enum ImportKind: Identifiable {
        case files, folder
        var id: Self { self }
        var types: [UTType] { self == .files ? [.item] : [.folder] }
    }

    var body: some View {
        if let snapshot = model.snapshot {
            content(snapshot)
                .onChange(of: snapshot.transfers.filter { $0.state == .complete }.count, initial: true) { old, new in
                    // "Landed." is a moment, not a state the core reports.
                    if new > completedCount, old != new, completedCount > 0 || old > 0 {
                        landed = true
                        Task {
                            try? await Task.sleep(for: .seconds(2.6))
                            landed = false
                        }
                    }
                    completedCount = new
                }
        }
    }

    private func content(_ snapshot: AppSnapshot) -> some View {
        let pads = snapshot.pads
        let active = snapshot.activeTransfers.filter { $0.state != .receiving }
        let sending = !active.isEmpty
        let selected = pads.filter { model.selectedPadIds.contains($0.id) }
        let dark = selected.filter { !$0.online }.count
        let padWord = Format.plural(selected.count, "pad")
        // A dark pad waits rather than going now; the count says so.
        let held = dark > 0 ? " · \(dark) dark" : ""
        let title = sending ? "Going" : landed ? "Landed" : preparing ? "Loading" : "Send"
        let sub: String = if sending {
            padWord
        } else if landed {
            ""
        } else if pads.isEmpty {
            "No pads"
        } else if selected.isEmpty {
            "Pick a pad"
        } else {
            "\(padWord)\(held)"
        }

        return ZStack {
            Floor(energised: sending)
            VStack(spacing: 0) {
                ScreenHeader()
                Spacer(minLength: 12)
                Headline(title: title, sub: sub.isEmpty ? nil : sub)
                    .padding(.horizontal, 24)
                    .animation(.easeInOut(duration: 0.25), value: title)
                padChips(pads)
                    .padding(.top, 22)
                    .padding(.horizontal, 16)
                Spacer(minLength: 16)
                deck(snapshot: snapshot, active: active)
                    .padding(.bottom, 10)
                TransporterRig(phase: sending ? .go : landed ? .done : .idle)
                    .padding(.horizontal, 12)
                sendMenu
                    .padding(.top, 34)
                arrivals(snapshot)
                    .padding(.top, 18)
                Spacer(minLength: 8)
            }
        }
        .safeAreaInset(edge: .bottom) {
            NoticeBar().padding(.bottom, 6)
        }
        .animation(.snappy, value: model.notice)
        .photosPicker(isPresented: $showingPhotos, selection: $photoSelection, maxSelectionCount: nil, preferredItemEncoding: .current)
        .onChange(of: photoSelection) { _, items in
            guard !items.isEmpty else { return }
            photoSelection = []
            Task { await sendPhotos(items) }
        }
        .fileImporter(
            isPresented: Binding(get: { importing != nil }, set: { if !$0 { importing = nil } }),
            allowedContentTypes: importing?.types ?? [.item],
            allowsMultipleSelection: importing == .files
        ) { result in
            guard case .success(let urls) = result, !urls.isEmpty else { return }
            Task { await sendPicked(urls) }
        }
    }

    private func padChips(_ pads: [Pad]) -> some View {
        GlassEffectContainer(spacing: 8) {
            FlowLayout(spacing: 8) {
                if pads.isEmpty {
                    Text("No pads")
                        .font(Theme.sans(14))
                        .foregroundStyle(Theme.dimmer)
                        .padding(.horizontal, 16)
                        .padding(.vertical, 9)
                        .glassEffect(.regular, in: .capsule)
                } else {
                    ForEach(pads) { pad in
                        let picked = model.selectedPadIds.contains(pad.id)
                        Button {
                            withAnimation(.snappy(duration: 0.2)) { model.togglePad(pad.id) }
                        } label: {
                            HStack(spacing: 7) {
                                if !pad.online {
                                    Circle().strokeBorder(picked ? Theme.onAccent.opacity(0.6) : Theme.dimmer, lineWidth: 1)
                                        .frame(width: 6, height: 6)
                                }
                                Text(pad.name)
                            }
                            .font(Theme.sans(14, weight: picked ? .medium : .regular))
                            .foregroundStyle(picked ? Theme.onAccent : pad.online ? Theme.soft : Theme.dimmer)
                            .padding(.horizontal, 16)
                            .padding(.vertical, 9)
                            .contentShape(Capsule())
                        }
                        .buttonStyle(.plain)
                        .glassChip(selected: picked)
                        .accessibilityAddTraits(picked ? .isSelected : [])
                        .accessibilityHint(pad.online ? "" : "Dark — will wait until it wakes")
                    }
                }
            }
        }
    }

    private struct Packet: Identifiable {
        var id: String
        var name: String
        var detail: String
        var glyph: String
    }

    private func deck(snapshot: AppSnapshot, active: [TransferBatch]) -> some View {
        // The deck carries what is actually moving; failing that, what is
        // staged and waiting for a recipient. An idle deck stays empty.
        let packets: [Packet] = !active.isEmpty
            ? active.prefix(3).map { Packet(id: $0.id, name: $0.label, detail: "\($0.progress)%", glyph: Self.glyph(for: $0.label)) }
            : model.stagedFiles.prefix(3).map { Packet(id: $0.path, name: $0.lastPathComponent, detail: "staged", glyph: Self.glyph(for: $0.lastPathComponent)) }
        return VStack(spacing: 6) {
            ForEach(Array(packets.enumerated()), id: \.element.id) { index, packet in
                HStack(spacing: 9) {
                    Image(systemName: packet.glyph)
                        .font(.system(size: 11, weight: .medium))
                        .foregroundStyle(Theme.accent)
                    Text(packet.name)
                        .font(Theme.mono(12.5))
                        .foregroundStyle(Theme.ink)
                        .lineLimit(1)
                        .truncationMode(.middle)
                    Spacer(minLength: 8)
                    Text(packet.detail)
                        .font(Theme.mono(12))
                        .foregroundStyle(Theme.dim)
                        .contentTransition(.numericText())
                }
                .padding(.horizontal, 12)
                .padding(.vertical, 9)
                .frame(width: 260)
                .background(Theme.packet)
                .overlay(Rectangle().strokeBorder(Theme.line(0.3), lineWidth: 1))
                .shadow(color: .black.opacity(0.55), radius: 11, y: 8)
                .phaseAnimator([false, true], trigger: active.isEmpty) { view, up in
                    view.offset(y: !active.isEmpty && up ? -7 : 0)
                } animation: { _ in .easeInOut(duration: 1.3).delay(Double(index) * 0.18) }
                .transition(.move(edge: .bottom).combined(with: .opacity))
            }
            if !model.stagedFiles.isEmpty, active.isEmpty {
                Button("Clear", role: .destructive) { withAnimation { model.clearStaged() } }
                    .buttonStyle(.mini)
            }
        }
        .animation(.snappy, value: packets.map(\.id))
    }

    private static func glyph(for name: String) -> String {
        let ext = (name as NSString).pathExtension.lowercased()
        if ext.isEmpty { return "folder" }
        if ["jpg", "jpeg", "png", "gif", "heic", "webp", "svg", "bmp", "tiff"].contains(ext) { return "photo" }
        if ["mov", "mp4", "m4v"].contains(ext) { return "video" }
        return "doc"
    }

    private var sendMenu: some View {
        Menu {
            Button("Photos & Videos", systemImage: "photo.on.rectangle") { showingPhotos = true }
            Button("Files", systemImage: "doc") { importing = .files }
            Button("Folder", systemImage: "folder") { importing = .folder }
            #if DEBUG
            // UI tests cannot drive the system pickers; they send a generated file.
            if ProcessInfo.processInfo.environment["FILEPORTER_UITEST"] != nil {
                Button("Sample file", systemImage: "testtube.2") { Task { await sendSample() } }
            }
            #endif
        } label: {
            Label(preparing ? "Loading…" : "Send", systemImage: "arrow.up")
                .font(Theme.sans(16, weight: .medium))
                .foregroundStyle(Theme.onAccent)
                .padding(.horizontal, 26)
                .padding(.vertical, 13)
                .contentShape(Capsule())
        }
        .glassEffect(.regular.tint(Theme.accent).interactive(), in: .capsule)
        .disabled(preparing)
        .accessibilityLabel("Send files or folders")
    }

    private func arrivals(_ snapshot: AppSnapshot) -> some View {
        let recent = snapshot.history
            .filter(\.incoming)
            .flatMap { entry in entry.items.filter { $0.state == .complete && $0.available }.map { (entry, $0) } }
            .prefix(2)
        return VStack(alignment: .leading, spacing: 6) {
            if !recent.isEmpty {
                Text("ARRIVED")
                    .font(Theme.sans(11))
                    .tracking(1.4)
                    .foregroundStyle(Theme.dimmer)
                    .padding(.leading, 4)
                ForEach(Array(recent), id: \.1.itemId) { entry, item in
                    HStack(spacing: 10) {
                        VStack(alignment: .leading, spacing: 2) {
                            Text(item.displayName)
                                .font(Theme.mono(13))
                                .foregroundStyle(Theme.value)
                                .lineLimit(1)
                                .truncationMode(.middle)
                            Text("\(Format.size(of: item)) · from \(Format.peer(entry.peerName, pads: snapshot.pads))")
                                .font(Theme.sans(12))
                                .foregroundStyle(Theme.dim)
                                .lineLimit(1)
                        }
                        Spacer(minLength: 8)
                        ArrivalMenu(item: item)
                    }
                    .padding(.leading, 14)
                    .padding(.trailing, 6)
                    .padding(.vertical, 6)
                    .glassEffect(.regular, in: .rect(cornerRadius: 22))
                }
            }
        }
        .padding(.horizontal, 20)
    }

    private func sendPicked(_ urls: [URL]) async {
        preparing = true
        defer { preparing = false }
        do {
            let copies = try model.outbox.adopt(urls)
            await model.send(copies)
        } catch {
            model.notice = .init(message: "Could not read that", bad: true)
        }
    }

    #if DEBUG
    private func sendSample() async {
        let name = "sample-\(UUID().uuidString.prefix(8)).txt"
        let url = URL.temporaryDirectory.appending(path: name)
        let body = String(repeating: "Fileporter end-to-end sample.\n", count: 20_000)
        do {
            try body.write(to: url, atomically: true, encoding: .utf8)
            await sendPicked([url])
        } catch {
            model.notice = .init(message: "Sample failed", bad: true)
        }
    }
    #endif

    private func sendPhotos(_ items: [PhotosPickerItem]) async {
        preparing = true
        defer { preparing = false }
        var copies: [URL] = []
        for (index, item) in items.enumerated() {
            guard let media = try? await item.loadTransferable(type: PickedMedia.self) else { continue }
            // A Live Photo arrives as an `.pvt` package holding the still, the
            // movie and a plist. Send the picture, not the packaging.
            let picked = PickedMedia.primaryFile(in: media.url) ?? media.url
            let name = PickedMedia.displayName(for: picked, index: index)
            if let copy = try? model.outbox.adoptTemporary(picked, preferredName: name) {
                copies.append(copy)
            }
            try? FileManager.default.removeItem(at: media.url.deletingLastPathComponent())
        }
        guard !copies.isEmpty else {
            model.notice = .init(message: "Could not load those photos", bad: true)
            return
        }
        await model.send(copies)
    }
}

/// A photo or video exported from the library as its original file.
nonisolated struct PickedMedia: Transferable {
    var url: URL

    static var transferRepresentation: some TransferRepresentation {
        FileRepresentation(importedContentType: .item) { received in
            // The received file is removed when this closure returns.
            let folder = URL.temporaryDirectory.appending(path: UUID().uuidString, directoryHint: .isDirectory)
            try FileManager.default.createDirectory(at: folder, withIntermediateDirectories: true)
            let destination = folder.appending(path: received.file.lastPathComponent)
            try FileManager.default.copyItem(at: received.file, to: destination)
            return PickedMedia(url: destination)
        }
    }

    /// Resolves what to actually send when Photos hands over a package rather
    /// than a file: the still image if there is one, else the movie.
    static func primaryFile(in url: URL) -> URL? {
        var isDirectory: ObjCBool = false
        guard FileManager.default.fileExists(atPath: url.path, isDirectory: &isDirectory), isDirectory.boolValue else {
            return nil
        }
        let contents = (try? FileManager.default.contentsOfDirectory(at: url, includingPropertiesForKeys: nil)) ?? []
        let images = ["heic", "heif", "jpg", "jpeg", "png", "gif", "tiff", "webp", "dng", "raw"]
        let movies = ["mov", "mp4", "m4v", "avi"]
        if let image = contents.first(where: { images.contains($0.pathExtension.lowercased()) }) {
            return image
        }
        return contents.first { movies.contains($0.pathExtension.lowercased()) }
    }

    /// Photos hands over names like `IMG_0042.HEIC`, or an opaque identifier.
    /// Replace an identifier with a readable, sortable name.
    static func displayName(for url: URL, index: Int) -> String {
        let name = url.lastPathComponent
        let stem = (name as NSString).deletingPathExtension
        let ext = (name as NSString).pathExtension
        let opaque = UUID(uuidString: stem) != nil || stem.count >= 30 || stem.isEmpty
        guard opaque else { return name }
        let stamp = Date.now.formatted(.iso8601.year().month().day().dateSeparator(.dash).time(includingFractionalSeconds: false).timeSeparator(.omitted))
        let readable = "Photo \(stamp)\(index > 0 ? " \(index + 1)" : "")"
        return ext.isEmpty ? readable : "\(readable).\(ext)"
    }
}

/// The transporter drawn as one object: beam, glow and ring are sized from the
/// pad, so it scales as a whole on any phone.
struct TransporterRig: View {
    enum Phase { case idle, go, done }
    var phase: Phase

    @State private var ringScale: CGFloat = 0.5
    @State private var ringOpacity: Double = 0
    @Environment(\.accessibilityReduceMotion) private var reduceMotion

    var body: some View {
        pad
            .frame(maxWidth: 520)
            .background {
                GeometryReader { proxy in
                    let size = proxy.size
                    // The pad's glowing core sits 49.47% up from its bottom edge.
                    let coreY = size.height * 0.5053
                    ZStack {
                        // A pool of light around the pad's core. The frame is
                        // square and wider than the gradient's own radius, so
                        // the light reaches nothing before its edge: a frame
                        // that cuts a gradient short draws a visible band.
                        RadialGradient(
                            gradient: Gradient(stops: [
                                .init(color: Theme.accent.opacity(phase == .idle ? 0.22 : 0.32), location: 0),
                                .init(color: Theme.accent.opacity(phase == .idle ? 0.08 : 0.12), location: 0.3),
                                .init(color: Theme.accent.opacity(0.02), location: 0.6),
                                .init(color: .clear, location: 1),
                            ]),
                            center: .center, startRadius: 0, endRadius: size.width * 1.1)
                            .frame(width: size.width * 2.2, height: size.width * 2.2)
                            .position(x: size.width / 2, y: coreY)
                        Beam(phase: phase, reduceMotion: reduceMotion)
                            .frame(width: size.width * 0.96, height: size.height * 2.3)
                            .position(x: size.width / 2, y: coreY - size.height * 1.15)
                    }
                }
                .allowsHitTesting(false)
            }
            .overlay {
                GeometryReader { proxy in
                    Ellipse()
                        .stroke(Theme.accent, lineWidth: 1.5)
                        .frame(width: proxy.size.width * 0.91, height: proxy.size.width * 0.91 * 132 / 620)
                        .scaleEffect(ringScale)
                        .opacity(ringOpacity)
                        .position(x: proxy.size.width / 2, y: proxy.size.height * 0.5053)
                }
                .allowsHitTesting(false)
            }
            .onChange(of: phase) { _, next in
                guard next == .done else { return }
                ringScale = 0.5
                ringOpacity = 0.85
                withAnimation(.timingCurve(0.22, 1, 0.36, 1, duration: 1)) {
                    ringScale = 1.12
                    ringOpacity = 0
                }
            }
            .accessibilityHidden(true)
    }

    /// The pad, with a bloom that follows its own artwork. A blurred copy of
    /// the whole layer would bloom its rectangle too, and the box shows on a
    /// black ground.
    private var pad: some View {
        PadArtView(art: .transport, timing: .transport, breath: phase == .idle ? 3.9 : 0.5)
            .shadow(color: Theme.accent.opacity(phase == .idle ? 0.5 : 0.75), radius: 14)
            .shadow(color: Theme.accent.opacity(phase == .idle ? 0.25 : 0.4), radius: 34)
    }
}

/// The light standing on the pad's core: a shaft that narrows into the pad and
/// fades out before its edges, so nothing ever draws a straight line.
private struct Beam: View {
    var phase: TransporterRig.Phase
    var reduceMotion: Bool

    /// Where the shaft's sides sit, as a fraction of the width: narrow where it
    /// meets the pad, wider in the air above it.
    private static let footHalf: CGFloat = 0.13
    private static let headHalf: CGFloat = 0.34

    var body: some View {
        let strength = phase == .idle ? 0.5 : 1.0
        let flow = phase == .idle ? 3.1 : 0.34
        TimelineView(.animation(minimumInterval: 1 / 30, paused: reduceMotion)) { timeline in
            Canvas { context, size in
                let time = timeline.date.timeIntervalSinceReferenceDate
                // Column by column: the shaft is drawn as a stack of thin
                // slices, each faded by how far it is from the centre. That is
                // what keeps it from reading as a rectangle with a mask on it.
                let steps = 40
                for step in 0..<steps {
                    let t = CGFloat(step) / CGFloat(steps - 1)
                    let offset = Self.footHalf + (Self.headHalf - Self.footHalf) * t
                    let falloff = pow(1 - t, 0.7)
                    let slice = Path { path in
                        path.move(to: CGPoint(x: size.width * (0.5 - offset), y: 0))
                        path.addLine(to: CGPoint(x: size.width * (0.5 + offset), y: 0))
                        path.addLine(to: CGPoint(x: size.width * 0.5, y: size.height))
                        path.closeSubpath()
                    }
                    context.fill(
                        slice,
                        with: .linearGradient(
                            Gradient(stops: [
                                .init(color: Theme.accent.opacity(0), location: 0),
                                .init(color: Theme.accent.opacity(0.035 * falloff), location: 0.55),
                                .init(color: Theme.accent.opacity(0.09 * falloff), location: 1),
                            ]),
                            startPoint: .zero, endPoint: CGPoint(x: 0, y: size.height)))
                }
                // Lines falling into the pad, quicker while something is on it.
                let spacing = size.height / 11
                let travel = reduceMotion ? 0 : CGFloat((time / flow).truncatingRemainder(dividingBy: 1)) * spacing
                var y = -spacing + travel
                while y < size.height {
                    let t = max(0, min(1, y / size.height))
                    let half = Self.headHalf + (Self.footHalf - Self.headHalf) * t
                    let fade = pow(t, 1.4)
                    context.fill(
                        Path(CGRect(
                            x: size.width * (0.5 - half * 0.72), y: y,
                            width: size.width * half * 1.44, height: 1.4)),
                        with: .color(Color(red: 205 / 255, green: 1, blue: 225 / 255).opacity(0.22 * fade)))
                    y += spacing
                }
            }
        }
        .blur(radius: 9)
        .opacity(strength)
        .blendMode(.plusLighter)
        .animation(.easeInOut(duration: 0.3), value: phase)
    }
}
