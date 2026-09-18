import Foundation

/// What each simulator calls itself while the UI tests run. Every suite uses
/// the same name for a given simulator: they share one device, and a pad that
/// is renamed keeps its old name on the other pad until the network's cached
/// record expires.
enum PadName {
    static let simulator = ProcessInfo.processInfo.environment["SIMULATOR_DEVICE_NAME"] ?? "Test pad"
    static let local = "Pad \(simulator)"
    /// The two-pad test runs on exactly these two simulators.
    static let other = simulator == "iPhone Air" ? "Pad iPhone 17 Pro" : "Pad iPhone Air"
}
