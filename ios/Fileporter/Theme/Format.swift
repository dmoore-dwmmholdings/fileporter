import Foundation

/// Presentation helpers, matching src/lib/format.ts so both apps read the same.
nonisolated enum Format {
    /// Renders a unix-second stamp relatively when recent, else in the locale.
    static func when(_ value: String, now: Date = .now) -> String {
        guard let seconds = TimeInterval(value.trimmingCharacters(in: .whitespaces)) else { return value }
        return when(Date(timeIntervalSince1970: seconds), now: now)
    }

    static func when(_ date: Date, now: Date = .now) -> String {
        let elapsed = now.timeIntervalSince(date)
        if elapsed >= 0, elapsed < 60 { return "Just now" }
        if elapsed >= 0, elapsed < 3600 { return "\(Int(elapsed / 60)) min ago" }
        let time = date.formatted(date: .omitted, time: .shortened)
        if Calendar.current.isDate(date, inSameDayAs: now) { return time }
        return "\(date.formatted(.dateTime.month(.abbreviated).day())), \(time)"
    }

    /// Sizes read the way a file manager writes them, not as raw byte counts.
    static func bytes(_ bytes: Int64) -> String {
        guard bytes >= 0 else { return "" }
        if bytes < 1000 { return "\(bytes) B" }
        let units = ["KB", "MB", "GB", "TB"]
        var value = Double(bytes) / 1000
        var unit = 0
        while value >= 1000, unit < units.count - 1 {
            value /= 1000
            unit += 1
        }
        let number = value < 10 ? String(format: "%.1f", value) : String(Int(value.rounded()))
        return "\(number) \(units[unit])"
    }

    /// Resolves a device id to the name the user gave it; an unknown id
    /// degrades to a short readable stem.
    static func peer(_ value: String, pads: [Pad]) -> String {
        if let match = pads.first(where: { $0.id == value }) { return match.name }
        if value.count <= 20 { return value }
        return "\(value.prefix(6))…\(value.suffix(4))"
    }

    /// Identities are long base32 strings; the board shows two readable groups.
    static func shortId(_ id: String) -> String {
        let upper = id.uppercased()
        return upper.count <= 9 ? upper : "\(upper.prefix(4)) · \(upper.suffix(4))"
    }

    static func plural(_ count: Int, _ word: String) -> String {
        "\(count) \(word)\(count == 1 ? "" : "s")"
    }

    /// A folder reads as what it holds and how big it is; a file as its size.
    static func size(of item: HistoryItem) -> String {
        guard item.kind == "directory" else { return bytes(item.size) }
        let files = plural(Int(item.itemCount ?? 0), "file")
        return item.size > 0 ? "\(files) · \(bytes(item.size))" : files
    }
}

nonisolated extension String {
    /// A dash reads better than an empty row in the diagnostics list.
    func ifEmpty(_ fallback: String) -> String { isEmpty ? fallback : self }
}
