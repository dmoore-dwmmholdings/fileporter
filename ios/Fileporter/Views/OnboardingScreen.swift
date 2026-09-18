import SwiftUI

/// First run. The pad is dim until the settings are saved and the listener is
/// actually advertising — the art is a readout, not decoration.
struct OnboardingScreen: View {
    @Environment(AppModel.self) private var model
    @State private var deviceName = ""
    @State private var notifications = true
    @State private var saving = false
    @State private var live = false
    @State private var error: String?
    @FocusState private var nameFocused: Bool

    private var validName: Bool {
        let trimmed = deviceName.trimmingCharacters(in: .whitespaces)
        return !trimmed.isEmpty && trimmed.count <= 48
    }

    var body: some View {
        ZStack {
            Floor(variant: .tall, energised: live)
            ScrollView {
                VStack(spacing: 30) {
                    HStack(spacing: 9) {
                        BrandMark()
                        Text("Fileporter").font(Theme.sans(14, weight: .medium)).foregroundStyle(Theme.ink)
                        Spacer()
                        HStack(spacing: 8) {
                            StatusDot(on: live)
                            Text(live ? "online · advertising" : "offline").font(Theme.sans(13)).foregroundStyle(Theme.dim)
                        }
                        .padding(.horizontal, 12)
                        .padding(.vertical, 7)
                        .glassEffect(.regular, in: .capsule)
                    }

                    Headline(title: live ? "Pad is live" : "Set up this pad")
                        .padding(.top, 20)

                    VStack(alignment: .leading, spacing: 22) {
                        VStack(alignment: .leading, spacing: 8) {
                            FieldLabel(text: "Name")
                            TextField("This iPhone", text: $deviceName)
                                .font(Theme.sans(17))
                                .foregroundStyle(Theme.ink)
                                .focused($nameFocused)
                                .submitLabel(.done)
                                .onSubmit(engage)
                                .underlinedField()
                                .onChange(of: deviceName) { _, name in
                                    if name.count > 48 { deviceName = String(name.prefix(48)) }
                                }
                        }

                        QuietToggle(label: "Notify on arrival", isOn: $notifications)

                        if let error {
                            Text(error).font(Theme.sans(13)).foregroundStyle(Theme.danger)
                        }

                        Button(action: engage) {
                            Text(live ? "Live" : saving ? "Starting…" : "Bring this pad online")
                                .font(Theme.sans(16, weight: .medium))
                                .frame(maxWidth: .infinity)
                                .padding(.vertical, 8)
                        }
                        .buttonStyle(.glassProminent)
                        .disabled(!validName || saving || live)

                    }
                    .frame(maxWidth: 460)
                }
                .padding(.horizontal, 24)
                .padding(.top, 8)
                .padding(.bottom, 200)
            }
            .scrollDismissesKeyboard(.interactively)
        }
        // The pad sits on the floor of the screen, not on top of the keyboard.
        .background(alignment: .bottom) {
            PadArtView(art: .transport, timing: .transport, breath: live ? 0.5 : 3.9)
                .opacity(live ? 1 : 0.42)
                .padding(.horizontal, -40)
                .offset(y: 40)
                .animation(.easeInOut(duration: 0.7), value: live)
                .allowsHitTesting(false)
                .ignoresSafeArea()
        }
        .background(Theme.background.ignoresSafeArea())
    }

    private func engage() {
        guard validName, !saving else {
            if !validName { error = "Name required" }
            return
        }
        saving = true
        error = nil
        nameFocused = false
        let name = deviceName.trimmingCharacters(in: .whitespaces)
        Task {
            do {
                try await model.completeOnboarding(deviceName: name, notificationsEnabled: notifications)
                live = true
            } catch {
                self.error = "Setup failed"
                saving = false
            }
        }
    }
}
