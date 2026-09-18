import XCTest

/// The Config screen's bottom bar is for acting, not for reading: when nothing
/// has been edited there is no bar at all. A permanent bar also fought the tab
/// bar's minimise gesture, because its height changed as the tab bar collapsed.
final class ConfigBarTests: XCTestCase {
    override func setUp() {
        continueAfterFailure = false
    }

    @MainActor
    func testTheApplyBarAppearsOnlyWhenSomethingChanged() throws {
        let app = XCUIApplication()
        app.launchEnvironment["FILEPORTER_UITEST"] = "1"
        app.launchEnvironment["FILEPORTER_DNSSD_SERVICE"] = "_fileporter-uitest._tcp"
        app.launch()

        let nameField = app.textFields["This iPhone"]
        if nameField.waitForExistence(timeout: 20) {
            nameField.tap()
            // The same name the two-pad test uses: these suites share a
            // simulator, and a pad renamed later keeps its old name on the
            // other pad until the network's cached record expires.
            nameField.typeText(PadName.local)
            app.buttons["Bring this pad online"].tap()
        }

        let config = app.tabBars.buttons["Config"]
        XCTAssertTrue(config.waitForExistence(timeout: 30))
        config.tap()

        let deviceName = app.textFields.element(boundBy: 0)
        XCTAssertTrue(deviceName.waitForExistence(timeout: 10))
        XCTAssertFalse(app.buttons["Apply"].exists, "settings in use need no bar")
        XCTAssertFalse(app.buttons["Discard"].exists)
        // The bar used to show the bound endpoint, which read as a control.
        let endpointish = NSPredicate(format: "label MATCHES %@", "^[0-9.]+:[0-9]+$")
        XCTAssertEqual(app.staticTexts.matching(endpointish).count, 0, "no stray endpoint label")
        add(screenshot(app, named: "config-clean"))

        deviceName.tap()
        deviceName.typeText(" 2")
        XCTAssertTrue(app.buttons["Apply"].waitForExistence(timeout: 5), "an edit needs Apply")
        XCTAssertTrue(app.buttons["Discard"].exists)
        add(screenshot(app, named: "config-dirty"))

        app.buttons["Discard"].tap()
        XCTAssertFalse(app.buttons["Apply"].waitForExistence(timeout: 3), "discarding clears the bar")
    }

    @MainActor
    private func screenshot(_ app: XCUIApplication, named name: String) -> XCTAttachment {
        let attachment = XCTAttachment(screenshot: app.screenshot())
        attachment.name = name
        attachment.lifetime = .keepAlways
        return attachment
    }
}
