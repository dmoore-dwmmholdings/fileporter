import SwiftUI
import UserNotifications

@main
struct FileporterApp: App {
    @State private var model = AppModel.shared
    @Environment(\.scenePhase) private var scenePhase

    var body: some Scene {
        WindowGroup {
            RootView()
                .environment(model)
                .task { await model.start() }
        }
        .onChange(of: scenePhase) { _, phase in
            model.scenePhaseChanged(to: phase)
        }
    }
}
