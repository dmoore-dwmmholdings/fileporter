import SwiftUI

/// The quiet direction, translated from src/styles/app.css: one accent, one
/// rgba ramp for every hairline, sans labels over mono values. Mono is kept for
/// what a machine produced — addresses, fingerprints, sizes, file names.
enum Theme {
    static let accent = Color(red: 116 / 255, green: 230 / 255, blue: 160 / 255)
    static let background = Color(red: 10 / 255, green: 16 / 255, blue: 13 / 255)
    static let ink = Color(red: 234 / 255, green: 246 / 255, blue: 239 / 255)
    static let soft = Color(red: 214 / 255, green: 229 / 255, blue: 219 / 255)
    static let value = Color(red: 179 / 255, green: 201 / 255, blue: 189 / 255)
    static let dim = Color(red: 138 / 255, green: 163 / 255, blue: 152 / 255)
    static let dimmer = Color(red: 111 / 255, green: 133 / 255, blue: 121 / 255)
    static let nav = Color(red: 127 / 255, green: 150 / 255, blue: 137 / 255)
    static let hold = Color(red: 224 / 255, green: 197 / 255, blue: 106 / 255)
    static let danger = Color(red: 232 / 255, green: 144 / 255, blue: 127 / 255)
    static let onAccent = Color(red: 7 / 255, green: 16 / 255, blue: 8 / 255)
    static let packet = Color(red: 12 / 255, green: 23 / 255, blue: 18 / 255)

    /// The accent at the opacities the desktop's `--line-*` tokens use.
    static func line(_ opacity: Double = 0.22) -> Color { accent.opacity(opacity) }

    static func sans(_ size: CGFloat, weight: Font.Weight = .regular, relativeTo style: Font.TextStyle = .body) -> Font {
        let name = switch weight {
        case .light, .thin, .ultraLight: "IBMPlexSans-Light"
        case .medium, .semibold, .bold, .heavy, .black: "IBMPlexSans-Medium"
        default: "IBMPlexSans-Regular"
        }
        return .custom(name, size: size, relativeTo: style)
    }

    static func mono(_ size: CGFloat, weight: Font.Weight = .regular, relativeTo style: Font.TextStyle = .body) -> Font {
        .custom(weight == .medium ? "IBMPlexMono-Medium" : "IBMPlexMono-Regular", size: size, relativeTo: style)
    }
}

/// The glow every screen shares, sitting on the floor behind the content.
struct Floor: View {
    enum Variant { case standard, tall, faint }
    var variant: Variant = .standard
    var energised = false

    var body: some View {
        GeometryReader { proxy in
            let height: CGFloat = variant == .tall ? 380 : 300
            RadialGradient(
                colors: [Theme.accent.opacity(variant == .faint ? 0.09 : 0.13), .clear],
                center: UnitPoint(x: 0.5, y: 1.26),
                startRadius: 0,
                endRadius: max(proxy.size.width, height) * 0.8
            )
            .frame(height: height)
            .frame(maxHeight: .infinity, alignment: .bottom)
            .opacity(energised ? 1 : 0.75)
            .animation(.easeInOut(duration: 0.22), value: energised)
        }
        .ignoresSafeArea()
        .allowsHitTesting(false)
        .accessibilityHidden(true)
    }
}

/// The 44px light headline and its dim line, scaled for a phone.
struct Headline: View {
    var title: String
    var sub: String?

    var body: some View {
        VStack(spacing: 10) {
            Text(title)
                .font(Theme.sans(38, weight: .light, relativeTo: .largeTitle))
                .tracking(-1.2)
                .foregroundStyle(Theme.ink)
                .accessibilityAddTraits(.isHeader)
            if let sub {
                Text(sub)
                    .font(Theme.sans(15, weight: .light))
                    .foregroundStyle(Theme.dim)
            }
        }
        .multilineTextAlignment(.center)
        .frame(maxWidth: .infinity)
        .contentTransition(.opacity)
    }
}

/// A small uppercase label over a value, as the desktop sets `.lbl`.
struct FieldLabel: View {
    var text: String
    var body: some View {
        Text(text).font(Theme.sans(12.5)).foregroundStyle(Theme.dim)
    }
}

struct Hint: View {
    var text: String
    var tone: Color = Theme.dimmer
    var body: some View {
        Text(text).font(Theme.sans(12)).foregroundStyle(tone).fixedSize(horizontal: false, vertical: true)
    }
}

/// Underlined fields instead of boxes.
struct UnderlinedFieldStyle: ViewModifier {
    var invalid = false
    @FocusState private var focused: Bool

    func body(content: Content) -> some View {
        content
            .focused($focused)
            .padding(.vertical, 7)
            .overlay(alignment: .bottom) {
                Rectangle()
                    .fill(invalid ? Theme.danger : focused ? Theme.accent : Theme.line(0.22))
                    .frame(height: 1)
            }
            .animation(.easeOut(duration: 0.2), value: focused)
    }
}

extension View {
    func underlinedField(invalid: Bool = false) -> some View {
        modifier(UnderlinedFieldStyle(invalid: invalid))
    }

    /// A Liquid Glass capsule. Picked chips take the accent the way the desktop
    /// fills `.pad-chip.on`.
    func glassChip(selected: Bool = false, interactive: Bool = true) -> some View {
        let glass: Glass = selected ? .regular.tint(Theme.accent.opacity(0.85)) : .regular
        return self.glassEffect(interactive ? glass.interactive() : glass, in: .capsule)
    }
}

/// The desktop's `.sw` switch, kept rather than the system toggle because the
/// screens are built around its thin outline.
struct QuietToggle: View {
    var label: String
    @Binding var isOn: Bool

    var body: some View {
        Button {
            withAnimation(.snappy(duration: 0.18)) { isOn.toggle() }
        } label: {
            HStack(spacing: 14) {
                Capsule()
                    .fill(Theme.accent.opacity(isOn ? 0.22 : 0.05))
                    .overlay(Capsule().strokeBorder(isOn ? Theme.accent : Theme.line(0.28), lineWidth: 1))
                    .overlay(alignment: isOn ? .trailing : .leading) {
                        Circle()
                            .fill(isOn ? Theme.accent : Theme.dim)
                            .frame(width: 14, height: 14)
                            .padding(3)
                    }
                    .frame(width: 40, height: 20)
                Text(label).font(Theme.sans(15)).foregroundStyle(Theme.ink)
                Spacer(minLength: 0)
            }
            .padding(.vertical, 9)
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .accessibilityRepresentation {
            Toggle(label, isOn: $isOn)
        }
    }
}

/// The small uppercase mono actions that sit on a row.
struct MiniActionStyle: ButtonStyle {
    var tone: Color = Theme.dim

    func makeBody(configuration: Configuration) -> some View {
        configuration.label
            .font(Theme.mono(11))
            .tracking(0.5)
            .textCase(.uppercase)
            .foregroundStyle(tone)
            .padding(.horizontal, 11)
            .padding(.vertical, 6)
            .overlay(Rectangle().strokeBorder(configuration.isPressed ? Theme.line(0.5) : Theme.line(0.22), lineWidth: 1))
            .contentShape(Rectangle())
            .opacity(configuration.isPressed ? 0.7 : 1)
    }
}

extension ButtonStyle where Self == MiniActionStyle {
    static var mini: MiniActionStyle { MiniActionStyle() }
    static var miniDanger: MiniActionStyle { MiniActionStyle(tone: Theme.danger) }
}

/// The status dot beside the device name.
struct StatusDot: View {
    var on: Bool
    var body: some View {
        Circle()
            .fill(on ? Theme.accent : .clear)
            .overlay(Circle().strokeBorder(on ? .clear : Theme.dimmer, lineWidth: 1))
            .frame(width: 6, height: 6)
    }
}

/// A beacon that breathes while a nearby pad is proving itself.
struct Beacon: View {
    var idle = false
    @State private var bright = false
    @Environment(\.accessibilityReduceMotion) private var reduceMotion

    var body: some View {
        Circle()
            .fill(idle ? .clear : Theme.accent)
            .overlay(Circle().strokeBorder(idle ? Theme.dimmer : .clear, lineWidth: 1))
            .frame(width: 7, height: 7)
            .opacity(idle || reduceMotion ? 1 : bright ? 1 : 0.3)
            .onAppear {
                guard !idle, !reduceMotion else { return }
                withAnimation(.easeInOut(duration: 1.2).repeatForever()) { bright = true }
            }
            .accessibilityHidden(true)
    }
}
