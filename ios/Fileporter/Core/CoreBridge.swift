import FileporterCore
import Foundation

/// A failure the core reported, carrying its stable code and safe message.
nonisolated struct CoreError: Error, Decodable, Sendable, LocalizedError {
    var code: String
    var message: String
    var retryable: Bool
    var field: String?

    var errorDescription: String? { message }

    static let undecodable = CoreError(
        code: "internal", message: "Core error", retryable: false)
}

/// Swift's side of src-tauri/src/mobile.rs. Every call blocks while the core
/// works, so calls run on a background queue and resume the caller after.
nonisolated enum CoreBridge {
    private static let queue = DispatchQueue(label: "io.fileporter.core", qos: .userInitiated, attributes: .concurrent)

    private struct Envelope<Value: Decodable>: Decodable {
        var ok: Value?
        var error: CoreError?
    }

    /// Accepts both `{"ok": null}` and a missing value for commands with no
    /// meaningful result.
    struct Empty: Decodable, Sendable {}

    static func start(dataDirectory: URL, receiveDirectory: URL, onChange: @escaping @convention(c) (UnsafeMutableRawPointer?, UInt64) -> Void) async throws {
        let data = dataDirectory.path
        let receive = receiveDirectory.path
        let reply: String = await run {
            take(fileporter_start(data, receive, onChange, nil))
        }
        let _: Empty? = try decode(reply, allowNull: true)
    }

    static func call<Value: Decodable & Sendable>(_ command: String, _ input: some Encodable & Sendable = [String: String](), as: Value.Type = Value.self) async throws -> Value {
        let json = String(decoding: try JSONEncoder().encode(input), as: UTF8.self)
        let reply: String = await run {
            take(fileporter_call(command, json))
        }
        guard let value: Value = try decode(reply, allowNull: false) else { throw CoreError.undecodable }
        return value
    }

    static func perform(_ command: String, _ input: some Encodable & Sendable = [String: String]()) async throws {
        let json = String(decoding: try JSONEncoder().encode(input), as: UTF8.self)
        let reply: String = await run {
            take(fileporter_call(command, json))
        }
        let _: Empty? = try decode(reply, allowNull: true)
    }

    private static func run(_ work: @escaping @Sendable () -> String) async -> String {
        await withCheckedContinuation { continuation in
            queue.async { continuation.resume(returning: work()) }
        }
    }

    private static func take(_ pointer: UnsafeMutablePointer<CChar>) -> String {
        defer { fileporter_free_string(pointer) }
        return String(cString: pointer)
    }

    private static func decode<Value: Decodable>(_ reply: String, allowNull: Bool) throws -> Value? {
        let data = Data(reply.utf8)
        if let object = try? JSONSerialization.jsonObject(with: data, options: [.fragmentsAllowed]) as? [String: Any] {
            if let error = object["error"], !(error is NSNull) {
                throw (try? JSONDecoder().decode(Envelope<Empty>.self, from: data).error) ?? CoreError.undecodable
            }
            if allowNull, object["ok"] == nil || object["ok"] is NSNull {
                return nil
            }
        }
        guard let envelope = try? JSONDecoder().decode(Envelope<Value>.self, from: data) else {
            if allowNull { return nil }
            throw CoreError.undecodable
        }
        return envelope.ok
    }
}
