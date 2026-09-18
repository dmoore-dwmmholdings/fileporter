import XCTest

/// Run on two simulators at once. Each onboards, waits to find and link the
/// other over Bonjour, sends it a generated file, and waits to receive one.
final class TwoPadTransportTests: XCTestCase {
    override func setUp() {
        continueAfterFailure = false
    }


    @MainActor
    func testTwoPadsLinkAndExchangeFiles() throws {
        let app = XCUIApplication()
        app.launchEnvironment["FILEPORTER_UITEST"] = "1"
        // Keep test pads off the real `_fileporter._tcp` service.
        app.launchEnvironment["FILEPORTER_DNSSD_SERVICE"] = "_fileporter-uitest._tcp"
        app.launch()

        let nameField = app.textFields["This iPhone"]
        if nameField.waitForExistence(timeout: 20) {
            nameField.tap()
            nameField.typeText(PadName.local)
            app.buttons["Bring this pad online"].tap()
        }

        let transport = app.tabBars.buttons["Transport"]
        XCTAssertTrue(transport.waitForExistence(timeout: 30), "Onboarding did not reach the tabs")
        // This pad may already be set up from an earlier run on the same
        // simulator; the other pad looks for it by name, so make sure it has
        // the one this test expects.
        nameThisPad(app)

        // The other simulator must prove its identity, be linked automatically
        // under the name it chose, and come online.
        let otherPad = app.buttons[PadName.other]
        XCTAssertTrue(otherPad.waitForExistence(timeout: 120), "\(PadName.other) never linked")
        let linked = NSPredicate(format: "label CONTAINS[c] '1 of 1 linked'")
        XCTAssertTrue(app.buttons.matching(linked).firstMatch.waitForExistence(timeout: 120), "\(PadName.other) never came online")

        let send = app.buttons["Send files or folders"]
        XCTAssertTrue(send.waitForExistence(timeout: 10))
        send.tap()
        app.buttons["Sample file"].tap()

        app.tabBars.buttons["Log"].tap()
        let sent = app.staticTexts["Sent"]
        let received = app.staticTexts["Received"]
        XCTAssertTrue(sent.waitForExistence(timeout: 120), "Nothing was sent")
        XCTAssertTrue(received.waitForExistence(timeout: 180), "Nothing arrived from the other pad")
        let verified = app.staticTexts.matching(NSPredicate(format: "label == 'Verified'"))
        let deadline = Date.now.addingTimeInterval(120)
        while verified.count < 2, Date.now < deadline {
            _ = verified.element(boundBy: 1).waitForExistence(timeout: 5)
        }
        XCTAssertGreaterThanOrEqual(verified.count, 2, "Both transports should verify end to end")
        let shot = XCTAttachment(screenshot: app.screenshot())
        shot.lifetime = .keepAlways
        add(shot)
    }

    /// Renames this pad through Config unless it already carries the name the
    /// other simulator looks for. A pad set up by an earlier run on the same
    /// simulator keeps whatever name that run gave it.
    @MainActor
    private func nameThisPad(_ app: XCUIApplication) {
        app.tabBars.buttons["Config"].tap()
        let field = app.textFields.element(boundBy: 0)
        guard field.waitForExistence(timeout: 10), (field.value as? String) != PadName.local else {
            app.tabBars.buttons["Transport"].tap()
            return
        }
        field.tap()
        // Delete what is there a character at a time: the selection menu is
        // not always up by the time a tap would reach it, and a missed Select
        // All appends instead of replacing.
        if let current = field.value as? String, !current.isEmpty {
            field.typeText(String(repeating: XCUIKeyboardKey.delete.rawValue, count: current.count))
        }
        field.typeText(PadName.local)
        app.buttons["Apply"].tap()
        XCTAssertFalse(app.buttons["Apply"].waitForExistence(timeout: 10), "the rename did not apply")
        app.tabBars.buttons["Transport"].tap()
    }
}
