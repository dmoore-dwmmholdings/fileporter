import Foundation
import Testing
@testable import Fileporter

/// The snapshot JSON the Rust core emits, trimmed to one of each record.
private let snapshotJSON = """
{"revision":7,"lifecycle":{"windowVisible":true,"receivingEnabled":true,"listening":true,"receiving":true,"boundEndpoint":"192.168.1.20:52011","shuttingDown":false},
"settings":{"deviceName":"Pocket","receiveDirectory":"/tmp/Received","onboardingComplete":true,"launchAtLogin":false,"notificationsEnabled":true,"automaticDeviceTrust":true,"receivingEnabled":true,"preferredListenAddress":"0.0.0.0:0","preferredListenPort":0,"historyRetentionDays":30},
"managersStarted":true,"localDeviceName":"Pocket",
"devices":[{"id":"dev-a","name":"Office","state":"online","lastSeenAt":1700000000}],
"nearbyDevices":[{"deviceId":"dev-b","displayName":"Laptop","endpoint":"192.168.1.30:48721","certificateFingerprint":"blake3:ff","protocolVersion":1,"capabilities":["receive-v1"]}],
"transfers":[{"id":"b1","label":"photo.heic","state":"sending","progress":42,"targets":[{"id":"t1","deviceName":"Office","state":"sending","progress":42,"rateLabel":"12 MB/s"}]}],
"history":[{"id":"b0","direction":"incoming","peerName":"dev-a","summary":"1 item","timeLabel":"1700000000","state":"completed","items":[{"itemId":"i0","displayName":"report.pdf","kind":"file","size":2048,"state":"completed","available":true},{"itemId":"i1","displayName":"IMG_0455.pvt","kind":"directory","size":6749517,"itemCount":3,"state":"completed","available":true}]}],
"queuedBatches":[{"id":"q1","itemCount":2,"targetDeviceIds":["dev-a"],"state":"queued","waitingForAvailable":true}],
"pairing":{"localDeviceId":"abcdefghijklmnop","pendingPairings":[{"id":"p1","deviceId":"dev-b","remoteName":"Laptop","certificateFingerprint":"blake3:ff","expiresAt":1700000120,"localConfirmed":false,"remoteConfirmed":true,"sasCode":"123456"}],
"trustedDevices":[{"deviceId":"dev-a","name":"Office","alias":"Studio","pairedAt":1690000000,"lastSeenAt":1700000000,"certificateFingerprintShort":"ab:cd","autoSend":true,"endpoint":"192.168.1.10:48721"}]},
"network":{"listening":true,"boundEndpoint":"192.168.1.20:52011","preferredListenAddress":"0.0.0.0:0","trustedOnlineEndpoints":[],"mdnsState":"advertising","localInterfaceSummaries":[],"recentErrorCodes":[]},
"about":{"appVersion":"0.1.2","protocolVersion":1,"logsAvailable":true,"databaseMigrationVersion":14,"ownedStagingBytes":0}}
"""

@MainActor
struct SnapshotDecodingTests {
    @Test func decodesTheCoreSnapshot() throws {
        let snapshot = try JSONDecoder().decode(AppSnapshot.self, from: Data(snapshotJSON.utf8))
        #expect(snapshot.revision == 7)
        #expect(snapshot.pads.count == 1)
        #expect(snapshot.pads[0].name == "Studio")
        #expect(snapshot.pads[0].online)
        #expect(snapshot.activeTransfers.map(\.id) == ["b1"])
        #expect(snapshot.history[0].state == .complete)
        #expect(snapshot.history[0].incoming)
        #expect(snapshot.pairing.pendingPairings[0].sasCode == "123456")
    }

    @Test func normalisesPersistedBatchStates() {
        #expect(BatchState(raw: "completed") == .complete)
        #expect(BatchState(raw: "receiving").inFlight)
        #expect(BatchState(raw: "something-new") == .failed)
        #expect(BatchState(raw: "waiting").word == "Held")
    }
}

@MainActor
struct FormatTests {
    @Test func foldersReadAsWhatTheyHold() throws {
        // A folder row carries no bytes of its own: a Live Photo arrives as a
        // package of three files and used to read "0 items".
        let snapshot = try JSONDecoder().decode(AppSnapshot.self, from: Data(snapshotJSON.utf8))
        let items = snapshot.history[0].items
        #expect(Format.size(of: items[0]) == "2.0 KB")
        #expect(Format.size(of: items[1]) == "3 files · 6.7 MB")
    }

    @Test func bytesReadLikeAFileManager() {
        #expect(Format.bytes(999) == "999 B")
        #expect(Format.bytes(1_400_000) == "1.4 MB")
        #expect(Format.bytes(84_200_000) == "84 MB")
    }

    @Test func peersResolveToLocalNames() {
        let pads = [Pad(id: "dev-a", name: "Studio", online: true, lastSeenAt: nil, fingerprintShort: "", autoSend: false, endpoint: nil)]
        #expect(Format.peer("dev-a", pads: pads) == "Studio")
        #expect(Format.peer("ABCDEFGHIJKLMNOPQRSTUVWXYZ", pads: pads) == "ABCDEF…WXYZ")
        #expect(Format.shortId("abcdefghijk") == "ABCD · HIJK")
    }

    @Test func recentStampsReadRelatively() {
        let now = Date(timeIntervalSince1970: 1_700_000_600)
        #expect(Format.when("1700000590", now: now) == "Just now")
        #expect(Format.when("1700000000", now: now) == "10 min ago")
        #expect(Format.when("not a stamp", now: now) == "not a stamp")
    }

    @Test func listenAddressesNeedAPort() {
        #expect(ConfigScreen.validListenAddress("0.0.0.0:0"))
        #expect(ConfigScreen.validListenAddress("192.168.1.2:48721"))
        #expect(!ConfigScreen.validListenAddress("192.168.1.2"))
        #expect(!ConfigScreen.validListenAddress(":80"))
        #expect(!ConfigScreen.validListenAddress("10.0.0.1:70000"))
    }
}

struct SVGPathTests {
    @Test func arcsLandOnTheirEndpoints() {
        let path = SVGPath.parse("M18 96 A332 68 0 0 0 682 96")
        let box = path.boundingRect
        #expect(abs(box.minX - 18) < 0.5)
        #expect(abs(box.maxX - 682) < 0.5)
        // The lower half of the ellipse: it sweeps down to cy + ry.
        #expect(abs(box.maxY - 164) < 1)
    }
}
