import SwiftUI

/// The pad seen edge-on — the object every screen is built around. Geometry
/// lives in PadArt+Geometry.swift; this file draws it and runs the same
/// animations the desktop stylesheet keys: segments chase around the rim,
/// nodes blink, and the core breathes.
struct PadArt: Sendable {
    enum Paint: Sendable {
        case accent
        case hex(UInt32)

        func color(accent: Color) -> Color {
            switch self {
            case .accent: accent
            case .hex(let value):
                Color(
                    red: Double((value >> 16) & 0xff) / 255,
                    green: Double((value >> 8) & 0xff) / 255,
                    blue: Double(value & 0xff) / 255)
            }
        }
    }

    enum GradientName: Sendable { case deck, rim, core, pool }

    struct Stop: Sendable {
        var location: Double
        var color: Paint
        var opacity: Double
    }

    struct Gradient: Sendable {
        var radial: Bool
        var stops: [Stop]
    }

    enum Geometry: Sendable {
        case path(String)
        case ellipse(cx: CGFloat, cy: CGFloat, rx: CGFloat, ry: CGFloat)
        case rect(x: CGFloat, y: CGFloat, width: CGFloat, height: CGFloat, radius: CGFloat)
        case circle(cx: CGFloat, cy: CGFloat, r: CGFloat)
    }

    enum Fill: Sendable {
        case none
        case accent
        case hex(UInt32)
        case gradient(GradientName)
    }

    enum Animation: Sendable {
        case none
        case segment(Int)
        case node(Int)
        case core
        case pool
    }

    struct Shape: Sendable {
        var path: Path
        var fill: Fill
        /// Every stroke in the art is drawn in the accent.
        var strokeWidth: CGFloat?
        var opacity: Double
        var animation: Animation

        init(_ geometry: Geometry, fill: Fill, strokeWidth: CGFloat?, opacity: Double, animation: Animation) {
            switch geometry {
            case .path(let data): path = SVGPath.parse(data)
            case let .ellipse(cx, cy, rx, ry): path = Path(ellipseIn: CGRect(x: cx - rx, y: cy - ry, width: rx * 2, height: ry * 2))
            case let .rect(x, y, width, height, radius):
                path = Path(roundedRect: CGRect(x: x, y: y, width: width, height: height), cornerRadius: radius)
            case let .circle(cx, cy, r): path = Path(ellipseIn: CGRect(x: cx - r, y: cy - r, width: r * 2, height: r * 2))
            }
            self.fill = fill
            self.strokeWidth = strokeWidth
            self.opacity = opacity
            self.animation = animation
        }
    }

    var viewBox: CGSize
    var gradients: [GradientName: Gradient]
    var shapes: [Shape]
}

/// How the pad is lit: its timings follow the desktop's `.pad-segs` (transport)
/// or `.m-segs` (tile) rules.
struct PadTiming {
    var segmentCycle: Double
    var segmentStep: Double
    var segmentLit: Double
    var segmentPeak: Double
    var nodeStep: Double

    static let transport = PadTiming(segmentCycle: 2.2, segmentStep: 0.28, segmentLit: 0.22, segmentPeak: 0.8, nodeStep: 0.2)
    static let tile = PadTiming(segmentCycle: 1.9, segmentStep: 0.32, segmentLit: 0.24, segmentPeak: 0.85, nodeStep: 0.26)
}

struct PadArtView: View {
    var art: PadArt = .transport
    var timing: PadTiming = .transport
    /// A dark pad is unlit: segments and nodes hold still and dim.
    var dark = false
    /// Seconds per core breath; the desktop quickens it while a transport runs.
    var breath: Double = 3.9
    var accent: Color = Theme.accent

    @Environment(\.accessibilityReduceMotion) private var reduceMotion

    var body: some View {
        TimelineView(.animation(minimumInterval: 1 / 30, paused: dark || reduceMotion)) { timeline in
            Canvas { context, size in
                draw(in: &context, size: size, time: timeline.date.timeIntervalSinceReferenceDate)
            }
        }
        .aspectRatio(art.viewBox, contentMode: .fit)
        .opacity(dark ? 0.38 : 1)
        .accessibilityHidden(true)
    }

    private func draw(in context: inout GraphicsContext, size: CGSize, time: Double) {
        let scale = min(size.width / art.viewBox.width, size.height / art.viewBox.height)
        context.translateBy(
            x: (size.width - art.viewBox.width * scale) / 2,
            y: (size.height - art.viewBox.height * scale) / 2)
        context.scaleBy(x: scale, y: scale)
        let still = dark || reduceMotion

        for shape in art.shapes {
            var layer = context
            layer.opacity = shape.opacity * animatedOpacity(shape.animation, time: time, still: still)
            if let gradientName = shape.fillGradient, let gradient = art.gradients[gradientName] {
                fill(shape.path, with: gradient, in: &layer)
            } else if let color = shape.fillColor(accent: accent) {
                layer.fill(shape.path, with: .color(color))
            }
            if let width = shape.strokeWidth {
                layer.stroke(shape.path, with: .color(accent), lineWidth: width)
            }
        }
    }

    private func animatedOpacity(_ animation: PadArt.Animation, time: Double, still: Bool) -> Double {
        switch animation {
        case .none:
            return 1
        case .pool:
            return 0.6
        case .segment(let index):
            if still { return dark ? 0.1 : 0.4 }
            let phase = positiveModulo(time - Double(index) * timing.segmentStep, timing.segmentCycle) / timing.segmentCycle
            return phase < timing.segmentLit ? timing.segmentPeak : 0.14
        case .node(let index):
            if still { return dark ? 0.1 : 1 }
            let phase = positiveModulo(time - Double(index) * timing.nodeStep, 1.6) / 1.6
            return phase < 0.5 ? 1 : 0
        case .core:
            if still { return dark ? 0.2 : 0.7 }
            let phase = positiveModulo(time, breath) / breath
            // ease-in-out between the two keyframes, peaking mid-cycle.
            let wave = (1 - cos(phase * 2 * .pi)) / 2
            return 0.55 + 0.3 * wave
        }
    }

    private func fill(_ path: Path, with gradient: PadArt.Gradient, in context: inout GraphicsContext) {
        let box = path.boundingRect
        let stops = gradient.stops.map {
            SwiftUI.Gradient.Stop(color: $0.color.color(accent: accent).opacity($0.opacity), location: $0.location)
        }
        var layer = context
        layer.clip(to: path)
        if gradient.radial {
            // objectBoundingBox radial: a unit circle stretched over the box.
            layer.translateBy(x: box.midX, y: box.midY)
            layer.scaleBy(x: box.width / 2, y: box.height / 2)
            layer.fill(
                Path(CGRect(x: -1, y: -1, width: 2, height: 2)),
                with: .radialGradient(SwiftUI.Gradient(stops: stops), center: .zero, startRadius: 0, endRadius: 1))
        } else {
            layer.fill(
                Path(box),
                with: .linearGradient(
                    SwiftUI.Gradient(stops: stops),
                    startPoint: CGPoint(x: box.midX, y: box.minY),
                    endPoint: CGPoint(x: box.midX, y: box.maxY)))
        }
    }

    private func positiveModulo(_ value: Double, _ modulus: Double) -> Double {
        let result = value.truncatingRemainder(dividingBy: modulus)
        return result < 0 ? result + modulus : result
    }
}

private extension PadArt.Shape {
    var fillGradient: PadArt.GradientName? {
        if case .gradient(let name) = fill { return name }
        return nil
    }

    func fillColor(accent: Color) -> Color? {
        switch fill {
        case .none, .gradient: nil
        case .accent: accent
        case .hex(let value): PadArt.Paint.hex(value).color(accent: accent)
        }
    }
}

/// Parses the subset of SVG path data the pad art uses: absolute M, L, A, Z.
nonisolated enum SVGPath {
    static func parse(_ data: String) -> Path {
        var path = Path()
        var tokens = tokenize(data)[...]
        var command: Character = "M"
        var current = CGPoint.zero

        func number() -> CGFloat? {
            guard case .number(let value)? = tokens.first else { return nil }
            tokens.removeFirst()
            return value
        }

        while !tokens.isEmpty {
            if case .command(let next) = tokens.first {
                command = next
                tokens.removeFirst()
            }
            switch command {
            case "M":
                guard let x = number(), let y = number() else { return path }
                current = CGPoint(x: x, y: y)
                path.move(to: current)
                command = "L"
            case "L":
                guard let x = number(), let y = number() else { return path }
                current = CGPoint(x: x, y: y)
                path.addLine(to: current)
            case "A":
                guard let rx = number(), let ry = number(), let rotation = number(),
                    let large = number(), let sweep = number(), let x = number(), let y = number()
                else { return path }
                let end = CGPoint(x: x, y: y)
                addArc(to: &path, from: current, to: end, rx: rx, ry: ry, rotation: rotation, largeArc: large != 0, sweep: sweep != 0)
                current = end
            case "Z", "z":
                path.closeSubpath()
                if case .number = tokens.first { command = "L" } else if tokens.isEmpty { return path }
            default:
                return path
            }
        }
        return path
    }

    private enum Token {
        case command(Character)
        case number(CGFloat)
    }

    private static func tokenize(_ data: String) -> [Token] {
        var tokens: [Token] = []
        var buffer = ""
        func flush() {
            if let value = Double(buffer) { tokens.append(.number(CGFloat(value))) }
            buffer = ""
        }
        for character in data {
            if character.isLetter {
                flush()
                tokens.append(.command(character))
            } else if character == " " || character == "," {
                flush()
            } else if character == "-", !buffer.isEmpty {
                flush()
                buffer.append(character)
            } else {
                buffer.append(character)
            }
        }
        flush()
        return tokens
    }

    /// SVG endpoint arc to centre parameterisation (SVG 1.1, appendix F.6.5).
    private static func addArc(
        to path: inout Path, from start: CGPoint, to end: CGPoint,
        rx: CGFloat, ry: CGFloat, rotation: CGFloat, largeArc: Bool, sweep: Bool
    ) {
        guard rx != 0, ry != 0, start != end else {
            path.addLine(to: end)
            return
        }
        let phi = rotation * .pi / 180
        let cosPhi = cos(phi), sinPhi = sin(phi)
        let dx = (start.x - end.x) / 2, dy = (start.y - end.y) / 2
        let x1 = cosPhi * dx + sinPhi * dy
        let y1 = -sinPhi * dx + cosPhi * dy
        var rx = abs(rx), ry = abs(ry)
        let lambda = (x1 * x1) / (rx * rx) + (y1 * y1) / (ry * ry)
        if lambda > 1 {
            rx *= sqrt(lambda)
            ry *= sqrt(lambda)
        }
        let numerator = rx * rx * ry * ry - rx * rx * y1 * y1 - ry * ry * x1 * x1
        let denominator = rx * rx * y1 * y1 + ry * ry * x1 * x1
        var coefficient = sqrt(max(0, numerator / denominator))
        if largeArc == sweep { coefficient = -coefficient }
        let cx1 = coefficient * rx * y1 / ry
        let cy1 = -coefficient * ry * x1 / rx
        let center = CGPoint(
            x: cosPhi * cx1 - sinPhi * cy1 + (start.x + end.x) / 2,
            y: sinPhi * cx1 + cosPhi * cy1 + (start.y + end.y) / 2)

        func angle(_ ux: CGFloat, _ uy: CGFloat, _ vx: CGFloat, _ vy: CGFloat) -> CGFloat {
            let sign: CGFloat = ux * vy - uy * vx < 0 ? -1 : 1
            let dot = ux * vx + uy * vy
            let length = sqrt(ux * ux + uy * uy) * sqrt(vx * vx + vy * vy)
            return sign * acos(min(1, max(-1, dot / length)))
        }
        let theta = angle(1, 0, (x1 - cx1) / rx, (y1 - cy1) / ry)
        var delta = angle((x1 - cx1) / rx, (y1 - cy1) / ry, (-x1 - cx1) / rx, (-y1 - cy1) / ry)
        if !sweep, delta > 0 { delta -= 2 * .pi }
        if sweep, delta < 0 { delta += 2 * .pi }

        let transform = CGAffineTransform(translationX: center.x, y: center.y)
            .rotated(by: phi)
            .scaledBy(x: rx, y: ry)
        path.addRelativeArc(
            center: .zero, radius: 1,
            startAngle: .radians(Double(theta)), delta: .radians(Double(delta)),
            transform: transform)
    }
}
