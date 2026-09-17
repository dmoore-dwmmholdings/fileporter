// External diagnostic reader, not an app accessibility bridge.
// swiftc -module-cache-path /tmp/fileporter-swift-cache scripts/inspect-macos-accessibility.swift -o /tmp/fileporter-ax
// /tmp/fileporter-ax <window-owning PID> [--activate]
import Cocoa
import ApplicationServices

guard CommandLine.arguments.count >= 2, let pid = pid_t(CommandLine.arguments[1]), pid > 0 else {
    fputs("Usage: fileporter-ax PID [--activate]\n", stderr)
    exit(2)
}
let app = AXUIElementCreateApplication(pid)
print("reader_pid=\(getpid()) target_pid=\(pid) trusted=\(AXIsProcessTrusted())")
print("timeout_status=\(AXUIElementSetMessagingTimeout(app, 2).rawValue)")

func read(_ element: AXUIElement, _ attribute: String, _ path: String) -> CFTypeRef? {
    var value: CFTypeRef?
    let status = AXUIElementCopyAttributeValue(element, attribute as CFString, &value)
    guard status == .success else {
        print("AX_ERROR path=\(path) attribute=\(attribute) code=\(status.rawValue)")
        return nil
    }
    return value
}

var visited = 0
func walk(_ element: AXUIElement, _ path: String, _ depth: Int) {
    guard depth <= 24, visited < 1000 else { print("TRUNCATED \(path)"); return }
    visited += 1
    var fields: [String] = []
    // Optional leaf attributes can be absent; errors on structural reads below
    // are printed separately, rather than being described as an empty tree.
    for key in ["AXRole", "AXSubrole", "AXTitle", "AXDescription", "AXValue", "AXEnabled", "AXFocused", "AXSelected", "AXExpanded", "AXPosition", "AXSize"] {
        var value: CFTypeRef?
        if AXUIElementCopyAttributeValue(element, key as CFString, &value) == .success, let value {
            fields.append("\(key)=\(value)")
        }
    }
    print("\(path) \(fields.joined(separator: " | "))")
    if let children = read(element, "AXChildren", path) as? [AXUIElement] {
        for (index, child) in children.enumerated() {
            if visited >= 1000 { print("TRUNCATED \(path)"); break }
            walk(child, "\(path)/child[\(index)]", depth + 1)
        }
    }
}

func inspect(_ phase: String) {
    print("PHASE \(phase)")
    visited = 0
    var role: CFTypeRef?
    let status = AXUIElementCopyAttributeValue(app, "AXRole" as CFString, &role)
    print("application_role_status=\(status.rawValue) role=\(String(describing: role))")
    if let windows = read(app, "AXWindows", "application") as? [AXUIElement] {
        print("windows_count=\(windows.count)")
        for (index, window) in windows.enumerated() { walk(window, "window[\(index)]", 0) }
    }
}

let windows = CGWindowListCopyWindowInfo(.optionAll, kCGNullWindowID) as? [[String: Any]] ?? []
for window in windows where (window[kCGWindowOwnerPID as String] as? Int) == Int(pid) {
    let keys = [kCGWindowNumber, kCGWindowOwnerPID, kCGWindowName, kCGWindowBounds, kCGWindowIsOnscreen]
    print("CG_WINDOW \(Dictionary(uniqueKeysWithValues: keys.compactMap { key in window[key as String].map { (key as String, $0) } }))")
}
inspect("before_activation")
if CommandLine.arguments.contains("--activate") {
    print("activate=\(NSRunningApplication(processIdentifier: pid)?.activate(options: []) ?? false)")
    RunLoop.current.run(until: Date().addingTimeInterval(1))
    inspect("after_activation_1s")
    RunLoop.current.run(until: Date().addingTimeInterval(3))
    inspect("after_activation_4s")
}
